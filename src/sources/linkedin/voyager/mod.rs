//! Authenticated content search against LinkedIn's internal Voyager API — the
//! same calls the web app makes, with your session cookie.
//!
//! This is what surfaces the personal "we're hiring, DM me" posts that never
//! become formal listings, and it is the highest-value and highest-risk source
//! here. Everything in this directory is shaped by two facts:
//!
//! 1. **It bans accounts.** Request volume is the thing to economise, which is
//!    why the walk goes deep once and shallow forever after (`service`).
//! 2. **It changes without notice.** The `queryId` and the response shape move
//!    when LinkedIn ships, so failures have to explain themselves precisely
//!    enough to fix from the dashboard (`client`, `parse`).

mod client;
mod parse;
mod service;

pub use service::Voyager;
