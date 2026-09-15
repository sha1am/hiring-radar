//! What every source needs, in one place.
//!
//! Each site gets its own directory because fetching from Greenhouse and
//! fetching from LinkedIn are genuinely different jobs — one is public JSON you
//! can hammer, the other is an authenticated private API that bans accounts.
//! Pretending they share a strategy produces a lowest-common-denominator client
//! that does neither well.
//!
//! But they do share plumbing: trimming a reqwest error into something a
//! dashboard can show, turning a status code into an explanation, stripping
//! HTML out of a description, finding the apply-to email in a wall of text,
//! reading a company list off disk. That lives here, so a fix lands once
//! instead of three times.

pub mod companies;
pub mod html;
pub mod http;
pub mod text;

pub use html::strip_html;
pub use http::{brief, status_hint};
pub use text::{extract_email, first_line, pretty_slug};
