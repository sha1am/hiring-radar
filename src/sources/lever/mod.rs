//! Lever job boards.
//!
//! Public JSON, no auth, same shape of source as Greenhouse — and the ATS a
//! large share of Indian startups actually use, which Greenhouse alone misses
//! entirely.

mod client;
mod parse;
mod service;

pub use service::Lever;
