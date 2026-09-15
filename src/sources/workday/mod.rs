//! Workday careers sites.
//!
//! Workday hosts a separate careers site per customer, which is why this source
//! is driven by a list of tenants rather than a search. It is the best route to
//! the large employers with real India engineering headcount — the ones whose
//! openings never appear on a startup-oriented board.
//!
//! Two undocumented traps shape the code here, both of which fail *silently*:
//!
//! 1. `limit` is capped at 20. Ask for more and you get an HTTP 400, or a 200
//!    with zero rows that reads as "this company isn't hiring". See
//!    [`client::PAGE_SIZE`].
//! 2. `total` is correct on page one and collapses to 0 on every page after, so
//!    the obvious `while fetched < total` loop stops after twenty jobs and
//!    reports it as the whole company. The crawl pages until a page comes back
//!    short instead. See [`service`].

mod client;
mod parse;
mod service;
pub mod target;

pub use service::Workday;
