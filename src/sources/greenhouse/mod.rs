//! Greenhouse job boards.
//!
//! Public JSON, no auth, no ban risk — this is the source you point at first to
//! prove the pipeline works end to end, and the one that keeps working when
//! LinkedIn changes its mind.
//!
//! Split three ways, the same as every other site here:
//! - `client` talks to the API and knows its wire format and nothing else
//! - `parse` turns that wire format into a `RawPost`
//! - `service` is the crawl strategy: which boards, in what order, what to say
//!   when one of them fails

mod client;
mod parse;
mod service;

pub use service::Greenhouse;
