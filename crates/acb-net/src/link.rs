//! A UDP socket with optional simulated loss, delay and jitter
//! (`--netsim loss=10%,delay=80ms,jitter=10ms`), applied to packets in
//! both directions on the side that sets it.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::proto::MAX_PACKET;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetSim {
    /// Fraction of packets dropped, 0..1.
    pub loss: f32,
    /// One-way delay added.
    pub delay: Duration,
    /// Extra random delay, 0..jitter (packets may reorder).
    pub jitter: Duration,
}

impl NetSim {
    /// `loss=10%,delay=80ms,jitter=10ms` (any subset; loss also as 0.1).
    pub fn parse(spec: &str) -> anyhow::Result<NetSim> {
        let mut n = NetSim::default();
        for item in spec.split(',').filter(|s| !s.is_empty()) {
            let (k, v) = item.split_once('=').ok_or_else(|| anyhow::anyhow!("--netsim: {item}: want key=value"))?;
            let ms = |v: &str| -> anyhow::Result<Duration> {
                let v = v.trim_end_matches("ms");
                Ok(Duration::from_secs_f64(v.parse::<f64>()? / 1000.0))
            };
            match k {
                "loss" => {
                    n.loss = match v.strip_suffix('%') {
                        Some(p) => p.parse::<f32>()? / 100.0,
                        None => v.parse::<f32>()?,
                    }
                }
                "delay" => n.delay = ms(v)?,
                "jitter" => n.jitter = ms(v)?,
                _ => anyhow::bail!("--netsim: unknown {k} (loss, delay, jitter)"),
            }
        }
        Ok(n)
    }

    pub fn active(&self) -> bool {
        self.loss > 0.0 || !self.delay.is_zero() || !self.jitter.is_zero()
    }
}

/// Traffic counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub sent: u64,
    pub sent_bytes: u64,
    pub recv: u64,
    pub recv_bytes: u64,
    /// Dropped by the simulator.
    pub dropped: u64,
}

pub struct Link {
    sock: UdpSocket,
    sim: NetSim,
    rng: u64,
    /// Delayed packets: (release time, peer, bytes, outgoing).
    queue: VecDeque<(Instant, SocketAddr, Vec<u8>, bool)>,
    pub stats: Stats,
}

impl Link {
    pub fn new(sock: UdpSocket, sim: NetSim, seed: u64) -> std::io::Result<Link> {
        sock.set_nonblocking(false)?;
        Ok(Link { sock, sim, rng: seed | 1, queue: VecDeque::new(), stats: Stats::default() })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    fn rand(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Whether the simulator lets a packet through, and when.
    fn route(&mut self) -> Option<Instant> {
        if self.sim.loss > 0.0 && self.rand() < self.sim.loss {
            self.stats.dropped += 1;
            return None;
        }
        let jitter = self.sim.jitter.mul_f32(self.rand());
        Some(Instant::now() + self.sim.delay + jitter)
    }

    fn enqueue(&mut self, at: Instant, peer: SocketAddr, data: Vec<u8>, out: bool) {
        // keep release order (jitter may put it before others)
        let i = self.queue.iter().rposition(|e| e.0 <= at).map_or(0, |i| i + 1);
        self.queue.insert(i, (at, peer, data, out));
    }

    pub fn send(&mut self, data: &[u8], peer: SocketAddr) {
        self.stats.sent += 1;
        self.stats.sent_bytes += data.len() as u64 + 28; // + IPv4/UDP headers
        if !self.sim.active() {
            let _ = self.sock.send_to(data, peer);
            return;
        }
        if let Some(at) = self.route() {
            self.enqueue(at, peer, data.to_vec(), true);
            self.flush();
        }
    }

    /// Sends delayed outgoing packets that are due.
    fn flush(&mut self) {
        let now = Instant::now();
        while let Some(i) = self.queue.iter().position(|e| e.3 && e.0 <= now) {
            let (_, peer, data, _) = self.queue.remove(i).unwrap();
            let _ = self.sock.send_to(&data, peer);
        }
    }

    /// The next packet, waiting up to `timeout` for one.
    pub fn recv(&mut self, timeout: Duration) -> Option<(Vec<u8>, SocketAddr)> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; MAX_PACKET + 64];
        loop {
            self.flush();
            let now = Instant::now();
            if let Some(i) = self.queue.iter().position(|e| !e.3 && e.0 <= now) {
                let (_, peer, data, _) = self.queue.remove(i).unwrap();
                self.count_recv(data.len());
                return Some((data, peer));
            }
            // wake for the next delayed packet or the deadline
            let next = self.queue.iter().map(|e| e.0).min().map_or(deadline, |t| t.min(deadline));
            let wait = next.saturating_duration_since(now);
            if wait.is_zero() && now >= deadline {
                return None;
            }
            let _ = self.sock.set_read_timeout(Some(wait.max(Duration::from_micros(100))));
            match self.sock.recv_from(&mut buf) {
                Ok((n, peer)) => {
                    let data = buf[..n].to_vec();
                    if !self.sim.active() {
                        self.count_recv(n);
                        return Some((data, peer));
                    }
                    if let Some(at) = self.route() {
                        self.enqueue(at, peer, data, false);
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                // e.g. ICMP port unreachable from a peer that left
                Err(_) => {}
            }
            if Instant::now() >= deadline && !self.queue.iter().any(|e| !e.3 && e.0 <= Instant::now()) {
                self.flush();
                return None;
            }
        }
    }

    fn count_recv(&mut self, n: usize) {
        self.stats.recv += 1;
        self.stats.recv_bytes += n as u64 + 28;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_netsim() {
        let n = NetSim::parse("loss=10%,delay=80ms,jitter=5ms").unwrap();
        assert!((n.loss - 0.1).abs() < 1e-6);
        assert_eq!(n.delay, Duration::from_millis(80));
        assert_eq!(n.jitter, Duration::from_millis(5));
        assert_eq!(NetSim::parse("loss=0.25").unwrap().loss, 0.25);
        assert!(NetSim::parse("lag=3").is_err());
    }

    #[test]
    fn delayed_lossy_link() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = b.local_addr().unwrap();
        let mut a = Link::new(a, NetSim { loss: 0.5, delay: Duration::from_millis(30), jitter: Duration::ZERO }, 7).unwrap();
        let mut b = Link::new(b, NetSim::default(), 1).unwrap();
        let t = Instant::now();
        for i in 0..200u8 {
            a.send(&[i], to);
        }
        // nothing arrives before the delay
        assert!(b.recv(Duration::from_millis(15)).is_none());
        let mut got = 0;
        while a.recv(Duration::from_millis(5)).is_none() && t.elapsed() < Duration::from_millis(100) {}
        while let Some(_) = b.recv(Duration::from_millis(50)) {
            got += 1;
        }
        assert!(t.elapsed() >= Duration::from_millis(30));
        assert!((60..140).contains(&got), "{got} of 200 through at 50% loss");
        assert_eq!(a.stats.dropped + got, 200);
    }
}
