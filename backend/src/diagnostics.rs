//! In-memory diagnostic ring for the MCP connector.
//!
//! Events come from `tracing` (info and above). Each message is scrubbed
//! before it is stored: no tokens, no database URLs, no document bodies.
//! The buffer is process-local and capped.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const CAP: usize = 300;
const MSG_MAX: usize = 400;

static PR_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"pr_[A-Za-z0-9_\-]{8,}").expect("pr regex"));
static PG_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)postgres(?:ql)?://\S+").expect("pg regex"));
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)authorization\s*[:=]\s*\S+").expect("auth regex")
});

static BUF: LazyLock<Mutex<VecDeque<Stored>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

struct Stored {
    ts: String,
    level: &'static str,
    target: String,
    message: String,
    owner_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagEvent {
    pub ts: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Scrub secrets, then cap the length. Safe to call on any log line.
pub fn scrub(input: &str) -> String {
    let once = crate::ocr::redact_for_log(input);
    let once = PR_TOKEN.replace_all(&once, "[REDACTED]");
    let once = PG_URL.replace_all(&once, "[REDACTED]");
    let once = AUTH_HEADER.replace_all(&once, "authorization:[REDACTED]");
    truncate_chars(&once, MSG_MAX)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn lock() -> std::sync::MutexGuard<'static, VecDeque<Stored>> {
    BUF.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub fn record(level: &'static str, target: &str, owner_id: Option<i32>, message: &str) {
    let message = scrub(message);
    if message.is_empty() {
        return;
    }
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut buf = lock();
    if buf.len() >= CAP {
        buf.pop_front();
    }
    buf.push_back(Stored {
        ts,
        level,
        target: target.to_string(),
        message,
        owner_id,
    });
}

/// Newest first. `level` is `warn` (warn+error) or `info` (info+warn+error).
/// Events tagged with another owner are dropped. Untagged events stay.
pub fn recent(owner_id: i32, level: &str, limit: usize) -> Vec<DiagEvent> {
    let max_rank = if level.eq_ignore_ascii_case("info") { 3 } else { 2 };
    let limit = limit.clamp(1, 100);
    let buf = lock();
    buf.iter()
        .rev()
        .filter(|event| rank(event.level) <= max_rank)
        .filter(|event| event.owner_id.is_none() || event.owner_id == Some(owner_id))
        .take(limit)
        .map(|event| DiagEvent {
            ts: event.ts.clone(),
            level: event.level.to_string(),
            target: event.target.clone(),
            message: event.message.clone(),
        })
        .collect()
}

fn rank(level: &str) -> u8 {
    match level {
        "ERROR" => 1,
        "WARN" => 2,
        "INFO" => 3,
        _ => 4,
    }
}

#[cfg(test)]
pub fn clear_for_test() {
    lock().clear();
}

/// Collects info, warn, and error into the ring. Debug and trace are ignored.
pub struct DiagLayer;

struct MsgVisitor {
    message: String,
    owner_id: Option<i32>,
}

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let rendered = format!("{value:?}");
            self.message = rendered.trim_matches('"').to_string();
        } else if field.name() == "owner_id" {
            let rendered = format!("{value:?}");
            self.owner_id = rendered.parse().ok();
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "owner_id" {
            self.owner_id = i32::try_from(value).ok();
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "owner_id" {
            self.owner_id = i32::try_from(value).ok();
        }
    }
}

impl<S> Layer<S> for DiagLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > tracing::Level::INFO {
            return;
        }
        let mut visitor = MsgVisitor {
            message: String::new(),
            owner_id: None,
        };
        event.record(&mut visitor);
        if visitor.message.is_empty() {
            visitor.message = event.metadata().name().to_string();
        }
        let label = match level {
            tracing::Level::ERROR => "ERROR",
            tracing::Level::WARN => "WARN",
            _ => "INFO",
        };
        record(label, event.metadata().target(), visitor.owner_id, &visitor.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_hides_tokens_keys_and_database_urls() {
        let raw = "failed pr_abcdefghijklmnopqrstuvwxyz sk-supersecret bearer hunter2 postgres://user:pw@db/paperrust Authorization: Token pr_zzzzzzzzzzzz";
        let out = scrub(raw);
        assert!(!out.contains("pr_abc"));
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!out.contains("supersecret"));
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("postgres://"));
        assert!(!out.contains("user:pw"));
        assert!(out.contains("[REDACTED]") || out.contains("authorization:[REDACTED]"));
    }

    #[test]
    fn recent_keeps_this_owner_and_the_requested_level() {
        clear_for_test();
        record("INFO", "paperrust", Some(1), "info for one");
        record("WARN", "paperrust", Some(1), "warn for one");
        record("ERROR", "paperrust", Some(2), "other owner");
        record("ERROR", "paperrust", None, "process start");
        let warn = recent(1, "warn", 20);
        let texts: Vec<_> = warn.iter().map(|e| e.message.as_str()).collect();
        assert!(texts.contains(&"warn for one"));
        assert!(texts.contains(&"process start"));
        assert!(!texts.iter().any(|t| t.contains("info for one")));
        assert!(!texts.iter().any(|t| t.contains("other owner")));
        let info = recent(1, "info", 20);
        assert!(info.iter().any(|e| e.message == "info for one"));
        clear_for_test();
    }
}
