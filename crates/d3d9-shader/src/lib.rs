//! Direct3D 9 shader model 3 bytecode: parser, disassembler and WGSL
//! translator.
//!
//! Brotherhood ships its materials as precompiled `vs_3_0` / `ps_3_0`
//! permutations with CTAB constant tables. [`parse`] reads a blob into a
//! [`Program`]; [`wgsl::translate`] turns it into a WGSL module whose
//! constant registers live in uniform arrays, so the engine binds constants
//! by the names in [`Program::constants`].

mod disasm;
pub mod parse;
pub mod wgsl;

pub use parse::{parse, Program, Stage};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated bytecode")]
    Truncated,
    #[error("malformed: {0}")]
    Malformed(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}
