//! Library facade for the `dux` crate.
//!
//! `dux` is primarily a binary (`src/main.rs`), but a small set of
//! pure helpers are exposed here so they can be exercised from
//! `tests/*.rs` integration tests without re-implementing them.
//!
//! Keep this surface minimal — only modules that are pure, free of
//! global state, and useful in tests belong here.

pub mod sanitize;
