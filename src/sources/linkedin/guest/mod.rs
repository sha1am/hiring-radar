//! The public job-card endpoint LinkedIn serves without a login.

mod client;
mod parse;
mod service;

pub use service::LinkedInGuest;
