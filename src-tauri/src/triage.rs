//! Inbox triage engine: classify an email and extract action items.
//!
//! The LLM-facing contract is a strict JSON object. All the fragile parts —
//! building the prompt, parsing a model's (often messy) response into a typed
//! verdict, clamping bad categories, stripping code fences — are pure functions
//! here, unit-tested without any network. The orchestrator in `inbox.rs` owns
//! the HTTP/LLM/Gmail side and calls into this module for the brains.

use serde::{Deserialize, Serialize};

use crate::gmail_read::InboxMessage;

/// The triage bucket for a message. `Noise` = newsletters/promotions/receipts
/// that need no action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    ActionNeeded,
    FollowUp,
    Fyi,
    Noise,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::ActionNeeded => "action_needed",
            Category::FollowUp => "follow_up",
            Category::Fyi => "fyi",
            Category::Noise => "noise",
        }
    }

    /// Parse loosely — accept hyphen/space variants and unknowns fall back to
    /// `Fyi` (safe default: shown, but not flagged urgent, never dropped).
    pub fn parse_loose(s: &str) -> Category {
        match s.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "action_needed" | "action" | "actionneeded" => Category::ActionNeeded,
            "follow_up" | "followup" => Category::FollowUp,
            "noise" | "promotion" | "newsletter" | "receipt" | "spam" => Category::Noise,
            _ => Category::Fyi,
        }
    }
}

/// The model's verdict for one email, after validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub category: Category,
    /// One-line reason the user can scan (why it landed in this bucket).
    pub reason: String,
    /// Extracted to-dos — concrete, imperative, owner-attributed when clear.
    pub action_items: Vec<String>,
    /// A suggested plain-text reply body, when a reply is warranted. Empty for
    /// Noise/Fyi. The user edits before the draft is ever staged in Gmail.
    pub suggested_reply: String,
}

/// System prompt: strict, anti-hallucination, JSON-only. Mirrors the tone of
/// the follow-up prompt already in the app (extract only what's really there).
pub const TRIAGE_SYSTEM_PROMPT: &str = "\
You are an inbox triage assistant. You read one email and return a STRICT JSON \
object — nothing else, no prose, no code fences. The JSON shape is exactly:\n\
{\n\
  \"category\": one of \"action_needed\" | \"follow_up\" | \"fyi\" | \"noise\",\n\
  \"reason\": a short one-line explanation of the category,\n\
  \"action_items\": array of strings, each a concrete to-do; [] if none,\n\
  \"suggested_reply\": a plain-text reply body, or \"\" if no reply is warranted\n\
}\n\
Category meanings:\n\
- action_needed: the recipient must DO something (a request, a task, a decision).\n\
- follow_up: the recipient is owed a response, or promised something that's now due.\n\
- fyi: informational, worth reading, but no action required.\n\
- noise: newsletters, promotions, receipts, automated notifications.\n\
Rules: Extract ONLY action items that genuinely appear in the email — never \
invent tasks, dates, or commitments. Attribute an owner when the email names one. \
Write suggested_reply only for action_needed or follow_up, in the recipient's \
voice, concise and professional; otherwise use \"\". Output JSON ONLY.";

/// Build the user message for one email: the fields the model reasons over.
pub fn build_user_message(msg: &InboxMessage) -> String {
    format!(
        "From: {}\nTo: {}\nSubject: {}\nDate: {}\n\n{}",
        msg.from.trim(),
        msg.to.trim(),
        msg.subject.trim(),
        msg.date.trim(),
        if msg.body.trim().is_empty() {
            msg.snippet.trim()
        } else {
            msg.body.trim()
        }
    )
}

/// Strip a leading/trailing ```json fence or stray prose and return the JSON
/// substring (first `{` … last `}`). Models love to wrap JSON despite being
/// told not to; this makes parsing resilient.
pub fn extract_json(raw: &str) -> &str {
    let trimmed = raw.trim();
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if e >= s => &trimmed[s..=e],
        _ => trimmed,
    }
}

/// Parse a model response into a validated `Verdict`. Never errors on a
/// well-formed-but-odd model: unknown categories become `Fyi`, missing fields
/// become empty, and a totally unparseable response becomes a safe `Fyi` with
/// a diagnostic reason (fail-open to "shown for review", never silently drop).
pub fn parse_verdict(raw: &str) -> Verdict {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        category: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        action_items: Vec<String>,
        #[serde(default)]
        suggested_reply: String,
    }

    let json = extract_json(raw);
    match serde_json::from_str::<Wire>(json) {
        Ok(w) => {
            let category = Category::parse_loose(&w.category);
            // Guard: a reply only makes sense for actionable buckets.
            let suggested_reply = match category {
                Category::ActionNeeded | Category::FollowUp => w.suggested_reply.trim().to_string(),
                _ => String::new(),
            };
            // Clean action items: trim, drop blanks, dedupe-preserving-order.
            let mut seen = std::collections::HashSet::new();
            let action_items = w
                .action_items
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter(|s| seen.insert(s.clone()))
                .collect();
            Verdict {
                category,
                reason: w.reason.trim().to_string(),
                action_items,
                suggested_reply,
            }
        }
        Err(e) => Verdict {
            category: Category::Fyi,
            reason: format!("could not parse triage output: {e}"),
            action_items: Vec::new(),
            suggested_reply: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg() -> InboxMessage {
        InboxMessage {
            id: "m1".into(),
            thread_id: "t1".into(),
            from: "Brad <brad@x.com>".into(),
            to: "me@x.com".into(),
            subject: "Deck".into(),
            date: "Mon, 1 Jan 2026".into(),
            snippet: "snip".into(),
            body: "Can you send the Q3 deck by Friday?".into(),
        }
    }

    #[test]
    fn category_parse_is_loose() {
        assert_eq!(Category::parse_loose("action_needed"), Category::ActionNeeded);
        assert_eq!(Category::parse_loose("Action-Needed"), Category::ActionNeeded);
        assert_eq!(Category::parse_loose("follow up"), Category::FollowUp);
        assert_eq!(Category::parse_loose("newsletter"), Category::Noise);
        assert_eq!(Category::parse_loose("whatever"), Category::Fyi);
    }

    #[test]
    fn build_user_message_uses_body_over_snippet() {
        let m = build_user_message(&msg());
        assert!(m.contains("Subject: Deck"));
        assert!(m.contains("send the Q3 deck by Friday"));
    }

    #[test]
    fn extract_json_unwraps_code_fence() {
        let raw = "```json\n{\"category\":\"fyi\",\"reason\":\"x\"}\n```";
        assert_eq!(extract_json(raw), "{\"category\":\"fyi\",\"reason\":\"x\"}");
    }

    #[test]
    fn extract_json_unwraps_prose() {
        let raw = "Sure! Here you go: {\"category\":\"noise\"} hope that helps";
        assert_eq!(extract_json(raw), "{\"category\":\"noise\"}");
    }

    #[test]
    fn parse_clean_verdict() {
        let raw = r#"{"category":"action_needed","reason":"Brad asked for the deck",
            "action_items":["Send the Q3 deck to Brad by Friday"],
            "suggested_reply":"Hi Brad, I'll send the Q3 deck by Friday."}"#;
        let v = parse_verdict(raw);
        assert_eq!(v.category, Category::ActionNeeded);
        assert_eq!(v.action_items.len(), 1);
        assert!(v.suggested_reply.contains("Q3 deck"));
    }

    #[test]
    fn reply_suppressed_for_noise() {
        let raw = r#"{"category":"noise","reason":"promo","action_items":[],
            "suggested_reply":"this should be dropped"}"#;
        let v = parse_verdict(raw);
        assert_eq!(v.category, Category::Noise);
        assert!(v.suggested_reply.is_empty());
    }

    #[test]
    fn action_items_trimmed_and_deduped() {
        let raw = r#"{"category":"action_needed","reason":"r",
            "action_items":["  do X ","do X","", "do Y"],"suggested_reply":"ok"}"#;
        let v = parse_verdict(raw);
        assert_eq!(v.action_items, vec!["do X", "do Y"]);
    }

    #[test]
    fn unparseable_fails_open_to_fyi() {
        let v = parse_verdict("the model said something weird with no json");
        assert_eq!(v.category, Category::Fyi);
        assert!(v.reason.contains("could not parse"));
        assert!(v.action_items.is_empty());
    }

    #[test]
    fn unknown_category_becomes_fyi() {
        let v = parse_verdict(r#"{"category":"urgent","reason":"r"}"#);
        assert_eq!(v.category, Category::Fyi);
    }
}
