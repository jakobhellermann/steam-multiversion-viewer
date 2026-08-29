// TODO(ai-review): review for style and correctness
//! Exploratory, self-contained parser for FMOD Studio `.bank` files.
//!
//! A `.bank` is a RIFF/LIST chunk container (form type `"FEV "`) that
//! serializes the entire FMOD Studio mixer/event graph, plus one or more
//! embedded FSB5 blobs holding the actual audio. Unlike the FSB5 payload
//! itself (see the `fsbex` crate for decoding that), the container is
//! uncompressed — no LZX/LZ4 layer to unwrap.
//!
//! This module only decodes what's needed to browse a bank and play its
//! audio back:
//! - [`chunks::parse_bank`] walks the chunk tree generically (tag + size,
//!   recursing into `LIST` chunks) without understanding any `*BODY`
//!   payload — that part of the format is not self-describing, each node
//!   type is a fixed, FMOD-version-dependent binary struct.
//! - [`string_table::parse_string_table`] decodes the `STDT` chunk (only
//!   present when the sibling `.strings.bank` exists), resolving GUIDs to
//!   human-readable event/bus paths.
//! - [`bank::Bank::embedded_fsb5`] returns each embedded FSB5 blob,
//!   sliced out using the `SNDH` chunk's recorded offsets — not
//!   necessarily the same as a `SND ` chunk's own payload bounds, see
//!   `sound_data`.
//!
//! Byte-level layouts here were verified against the reverse-engineered
//! reference parser at <https://github.com/Masusder/FModBankParser>
//! (chunk tag values, the `X16` varint, and the radix-tree string table)
//! and cross-checked against real banks from a mounted depot — not
//! guessed.

pub mod bank;
pub mod chunks;
pub mod reader;
pub mod sound_data;
pub mod string_table;
pub mod tree;

pub use bank::{Bank, parse};
pub use chunks::Chunk;
pub use reader::{Reader, format_guid};
pub use string_table::StringTable;

#[cfg(test)]
mod tests;
