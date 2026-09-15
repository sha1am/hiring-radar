//! Recent warnings and errors, kept in memory and readable from the dashboard.
//!
//! `docker compose logs` already has all of this, and it is the wrong place to
//! send someone. The interesting lines are a handful of warnings scattered
//! through thousands of info records, they scroll away, and "paste me your
//! logs" turns into a chore that produces either nothing or four thousand
//! lines of noise.
//!
//! So warnings and errors are also captured here: a small ring buffer, exposed
//! at `/api/logs`, shown on the Status tab with a button that copies the lot.
//! One click, and the report is the fifty lines that actually matter.
//!
//! Deliberately memory-only and deliberately small. This is a diagnostic
//! convenience, not an audit trail — it must never be the reason a long-running
//! instance grows, and a restart clearing it is fine because a restart also
//! clears the problem it was describing.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// How many entries to keep. Enough to cover a few crawl cycles across every
/// source, small enough that it is never what fills a container's memory.
const CAPACITY: usize = 300;

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// Unix seconds.
    pub at: i64,
    /// "WARN" | "ERROR".
    pub level: String,
    /// The module that emitted it — `hiring_radar::sources::workday::service`.
    pub target: String,
    /// The event's message.
    pub message: String,
    /// Structured fields, already flattened to `key=value` — the part that
    /// usually carries the actual identifier you need.
    pub fields: String,
}

fn buffer() -> &'static Mutex<VecDeque<Entry>> {
    static BUF: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    BUF.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

/// Newest first, which is the order you read them in.
pub fn recent(limit: usize) -> Vec<Entry> {
    buffer()
        .lock()
        .map(|b| b.iter().rev().take(limit).cloned().collect())
        .unwrap_or_default()
}

pub fn clear() {
    if let Ok(mut b) = buffer().lock() {
        b.clear();
    }
}

/// The ring itself, over a buffer the caller owns.
///
/// Split out from the global so it can be tested without one: the tests used
/// to share the real buffer with every other test in the binary, several of
/// which log warnings, and the result was a failure that depended on which
/// tests ran first.
fn push_into(b: &mut VecDeque<Entry>, e: Entry) {
    if b.len() >= CAPACITY {
        b.pop_front();
    }
    b.push_back(e);
}

fn push(e: Entry) {
    if let Ok(mut b) = buffer().lock() {
        push_into(&mut b, e);
    }
}

/// Render a slice of entries as a report. Same split, same reason.
fn render(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "No warnings or errors recorded since start-up.".into();
    }
    let mut out = String::new();
    // Oldest first here, unlike the panel: a report reads as a sequence.
    for e in entries.iter().rev() {
        out.push_str(&format!(
            "{} {:5} {} — {}{}\n",
            e.at,
            e.level,
            e.target,
            e.message,
            if e.fields.is_empty() {
                String::new()
            } else {
                format!("  [{}]", e.fields)
            }
        ));
    }
    out
}

/// Flattens an event's fields into `message` and everything else.
#[derive(Default)]
struct Collector {
    message: String,
    fields: Vec<String>,
}

impl Visit for Collector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `message` is the event's own text; everything else is context that
        // usually carries the identifier — which board, which tenant, which id.
        if field.name() == "message" {
            self.message = format!("{value:?}").trim_matches('"').to_string();
        } else {
            self.fields.push(format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(format!("{}={}", field.name(), value));
        }
    }
}

/// A tracing layer that copies WARN and ERROR into the ring buffer.
///
/// Only those two levels. Info records are the crawl narrating itself, and
/// keeping them would bury the six lines someone actually needs to send.
pub struct CaptureLayer;

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > Level::WARN {
            return;
        }
        let mut c = Collector::default();
        event.record(&mut c);
        push(Entry {
            at: crate::model::now(),
            level: level.to_string(),
            target: event.metadata().target().to_string(),
            message: c.message,
            fields: c.fields.join(" "),
        });
    }
}

/// The whole buffer as plain text, for the copy button.
///
/// Rendered server-side so what lands in a bug report is the same everywhere,
/// and so the client never has to reimplement the formatting.
pub fn as_text(limit: usize) -> String {
    render(&recent(limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: i64) -> Entry {
        Entry {
            at: n,
            level: "WARN".into(),
            target: "hiring_radar::sources::workday::service".into(),
            message: format!("m{n}"),
            fields: String::new(),
        }
    }

    #[test]
    fn the_buffer_is_bounded() {
        // A diagnostic must never be the reason a long-running container grows.
        let mut b = VecDeque::new();
        for i in 0..(CAPACITY as i64 + 50) {
            push_into(&mut b, entry(i));
        }
        assert_eq!(b.len(), CAPACITY);
        // The oldest are the ones dropped.
        assert_eq!(b.front().unwrap().at, 50);
    }

    #[test]
    fn a_report_reads_oldest_first_even_though_the_panel_reads_newest_first() {
        // `recent` hands back newest-first for the panel, so the report has to
        // reverse it — a sequence of events told backwards is unreadable.
        let newest_first: Vec<Entry> = (0..3).rev().map(entry).collect();
        let text = render(&newest_first);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("m0"), "{text}");
        assert!(lines[2].contains("m2"), "{text}");
    }

    #[test]
    fn an_empty_buffer_says_so_rather_than_handing_back_nothing() {
        assert!(render(&[]).contains("No warnings or errors"));
    }

    #[test]
    fn fields_survive_because_they_carry_the_identifier() {
        // "workday page failed" is useless without knowing which tenant.
        let e = Entry {
            fields: "target=cisco:wd5:Cisco_Careers page=2".into(),
            message: "workday page failed".into(),
            ..entry(1)
        };
        let text = render(&[e]);
        assert!(text.contains("cisco:wd5"), "{text}");
        assert!(text.contains("workday page failed"), "{text}");
    }
}
