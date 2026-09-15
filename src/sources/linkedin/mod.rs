//! LinkedIn, which is two quite different sources wearing one name.
//!
//! `guest` is the public "see more job postings" endpoint — no login, HTML job
//! cards, formal listings. `voyager` is the private API the web app itself
//! calls — authenticated with your session cookie, JSON, and the only way to
//! reach the "we're hiring, DM me" posts that never become listings.
//!
//! They share a site, a rate-limit budget and a cookie convention, which is why
//! they share a directory and `auth`. They share nothing else: one can be
//! hammered, the other can get your account restricted.

pub mod auth;
pub mod guest;
pub mod voyager;

pub use guest::LinkedInGuest;
pub use voyager::Voyager;
