//! Reader for Assassin's Creed Brotherhood `.forge` archives.
//!
//! [`Forge`] indexes an archive and hands out raw entry payloads;
//! [`DataFile`] unpacks one `.data` payload into its typed sub-objects.
//! Decoding a sub-object's fields is the schema/fastload layer's job.

pub mod datafile;
pub mod forge;
mod io;
pub mod lzo2a;

pub use datafile::{DataFile, SubObject};
pub use forge::{Entry, Forge};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("format: {0}")]
    Format(String),
    #[error("lzo2a: {0}")]
    Lzo(&'static str),
}
