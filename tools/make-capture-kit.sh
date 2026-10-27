#!/usr/bin/env bash
# Builds a static acb-trace and packs dist/capture-kit.tar.gz (acb-trace, offsets.toml,
# capture.sh, README.md) for the retail Wine machine.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
musl=x86_64-unknown-linux-musl
if command -v rustup >/dev/null && ! rustup target list --installed | grep -qx $musl; then
  rustup target add $musl || true
fi
if [ -d "$(rustc --print sysroot)/lib/rustlib/$musl" ]; then
  cargo build --release -p acb-trace --target $musl
  bin=target/$musl/release/acb-trace
else
  echo "musl target not installed: building a static glibc binary instead" >&2
  RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p acb-trace --target x86_64-unknown-linux-gnu
  bin=target/x86_64-unknown-linux-gnu/release/acb-trace
fi
file "$bin" | grep -q 'statically linked\|static-pie' || echo "warning: $bin is not static" >&2
kit=$(mktemp -d)/capture-kit
mkdir -p "$kit" dist
cp "$bin" "$kit/acb-trace"
strip "$kit/acb-trace" 2>/dev/null || true
cp crates/acb-trace/offsets.toml tools/capture-kit/capture.sh tools/capture-kit/README.md "$kit/"
chmod +x "$kit/acb-trace" "$kit/capture.sh"
tar -C "$(dirname "$kit")" -czf dist/capture-kit.tar.gz capture-kit
echo "dist/capture-kit.tar.gz: $(tar -tzf dist/capture-kit.tar.gz | tr '\n' ' ')"
