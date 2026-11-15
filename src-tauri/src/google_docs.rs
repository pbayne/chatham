//! Export a note to a nicely formatted Google Doc (one click).
//!
//! Flow:
//!   1. Create an empty doc via the Docs API (`documents.create`) — owned by
//!      the user, under the `drive.file` scope (Humla only ever touches docs it
//!      creates, never the rest of Drive).
//!   2. Build a `batchUpdate` that inserts the note's text and applies real
//!      structure: the title as HEADING_1, each section (Summary / Transcript /
//!      Notes) as HEADING_2, bullet lists for `- ` lines, and bold runs for
//!      `**…**`.
//!
//! The formatting converter (`doc_requests`) is pure and unit-tested: it turns
//! a list of structured `DocBlock`s into the exact ordered Docs API requests,
//! tracking the 1-based character index the API uses so insert + style ranges
//! line up. The note→blocks step reuses the same section model as `export.rs`.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

const DOCS_CREATE: &str = "https://docs.googleapis.com/v1/documents";

/// A structured line of the document, before it becomes Docs API requests.
#[derive(Debug, Clone, PartialEq)]
pub enum DocBlock {
    /// Document title (HEADING_1).
    Title(String),
    /// Section heading (HEADING_2).
    Heading(String),
    /// A normal paragraph. Bold spans are marked with `**…**` in the text.
    Paragraph(String),
    /// A bullet-list item (one bullet). Bold spans as `**…**`.
    Bullet(String),
}

/// A span of text plus whether it's bold — the result of splitting `**…**`.
#[derive(Debug, Clone, PartialEq)]
struct Span {
    text: String,
    bold: bool,
}

/// Split a line on `**bold**` markers into alternating plain/bold spans.
fn split_bold(s: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("**") {
        if start > 0 {
            spans.push(Span { text: rest[..start].to_string(), bold: false });
        }
        let after = &rest[start + 2..];
        if let Some(end) = after.find("**") {
            spans.push(Span { text: after[..end].to_string(), bold: true });
            rest = &after[end + 2..];
        } else {
            // Unterminated ** — treat the rest as plain, markers included.
            spans.push(Span { text: rest[start..].to_string(), bold: false });
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        spans.push(Span { text: rest.to_string(), bold: false });
    }
    if spans.is_empty() {
        spans.push(Span { text: String::new(), bold: false });
    }
    spans
}

/// UTF-16 code-unit length — the unit the Docs API counts indices in. ASCII is
/// 1, BMP chars 1, astral (emoji) 2. Getting this wrong misaligns every range
/// after a multi-byte char, so we count explicitly rather than use `.len()`.
fn utf16_len(s: &str) -> i64 {
    s.chars().map(|c| c.len_utf16() as i64).sum()
}

/// Turn structured blocks into the ordered Docs API requests for a single
/// `batchUpdate`. The Docs API indexes an empty doc's body at 1. We insert each
/// block's text (plus a trailing newline) at a running cursor, then emit
/// paragraph-style and bullet and bold requests against the range we just
/// wrote, then advance the cursor. Pure + deterministic for testing.
pub fn doc_requests(blocks: &[DocBlock]) -> Vec<Value> {
    let mut reqs = Vec::new();
    let mut index: i64 = 1;

    for block in blocks {
        let (raw, style, bullet) = match block {
            DocBlock::Title(t) => (t.as_str(), Some("HEADING_1"), false),
            DocBlock::Heading(t) => (t.as_str(), Some("HEADING_2"), false),
            DocBlock::Paragraph(t) => (t.as_str(), None, false),
            DocBlock::Bullet(t) => (t.as_str(), None, true),
        };

        let spans = split_bold(raw);
        let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
        let text = format!("{plain}\n");
        let para_start = index;
        let para_end = index + utf16_len(&text); // includes the newline

        // 1. Insert the text.
        reqs.push(json!({
            "insertText": { "location": { "index": index }, "text": text }
        }));

        // 2. Paragraph style (heading level), applied to the paragraph range.
        if let Some(named) = style {
            reqs.push(json!({
                "updateParagraphStyle": {
                    "range": { "startIndex": para_start, "endIndex": para_end },
                    "paragraphStyle": { "namedStyleType": named },
                    "fields": "namedStyleType"
                }
            }));
        }

        // 3. Bullet.
        if bullet {
            reqs.push(json!({
                "createParagraphBullets": {
                    "range": { "startIndex": para_start, "endIndex": para_end },
                    "bulletPreset": "BULLET_DISC_CIRCLE_SQUARE"
                }
            }));
        }

        // 4. Bold runs: walk the spans, bolding only the bold ones.
        let mut run = para_start;
        for span in &spans {
            let len = utf16_len(&span.text);
            if span.bold && len > 0 {
                reqs.push(json!({
                    "updateTextStyle": {
                        "range": { "startIndex": run, "endIndex": run + len },
                        "textStyle": { "bold": true },
                        "fields": "bold"
                    }
                }));
            }
            run += len;
        }

        index = para_end;
    }

    reqs
}

/// Build the block list for a note. `include_*` mirror the export options.
pub fn blocks_for_note(
    title: &str,
    summary: &str,
    transcript: &str,
    include_summary: bool,
    include_transcript: bool,
) -> Vec<DocBlock> {
    let mut blocks = Vec::new();
    let t = if title.trim().is_empty() { "Untitled note" } else { title.trim() };
    blocks.push(DocBlock::Title(t.to_string()));

    if include_summary && !summary.trim().is_empty() {
        blocks.push(DocBlock::Heading("Summary".to_string()));
        for line in summary.trim().lines() {
            let l = line.trim_end();
            if l.is_empty() {
                continue;
            }
            // Markdown bullets (`- ` / `* `) become Docs bullets; `#` headings
            // from the model collapse to bold paragraphs (the doc already has
            // its own heading hierarchy).
            if let Some(rest) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
                blocks.push(DocBlock::Bullet(rest.to_string()));
            } else if let Some(rest) = l.strip_prefix("## ").or_else(|| l.strip_prefix("# ")) {
                blocks.push(DocBlock::Paragraph(format!("**{rest}**")));
            } else {
                blocks.push(DocBlock::Paragraph(l.to_string()));
            }
        }
    }

    if include_transcript && !transcript.trim().is_empty() {
        blocks.push(DocBlock::Heading("Transcript".to_string()));
        for line in transcript.trim().lines() {
            let l = line.trim_end();
            if !l.is_empty() {
                blocks.push(DocBlock::Paragraph(l.to_string()));
            }
        }
    }

    blocks
}

/// Create the doc and apply formatting. Returns the doc's URL.
pub async fn create_doc(
    http: &reqwest::Client,
    access_token: &str,
    title: &str,
    blocks: &[DocBlock],
) -> Result<String> {
    // 1. Create an empty doc with the title (shows in Drive + the tab).
    let create_resp = http
        .post(DOCS_CREATE)
        .bearer_auth(access_token)
        .json(&json!({ "title": title }))
        .send()
        .await
        .context("create doc request failed")?;
    let status = create_resp.status();
    let body = create_resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("docs create {status}: {body}"));
    }
    let doc: Value = serde_json::from_str(&body).context("decode create response")?;
    let doc_id = doc
        .get("documentId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("create response had no documentId"))?
        .to_string();

    // 2. batchUpdate the content.
    let requests = doc_requests(blocks);
    if !requests.is_empty() {
        let url = format!("{DOCS_CREATE}/{doc_id}:batchUpdate");
        let resp = http
            .post(&url)
            .bearer_auth(access_token)
            .json(&json!({ "requests": requests }))
            .send()
            .await
            .context("batchUpdate request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("docs batchUpdate {status}: {body}"));
        }
    }

    Ok(format!("https://docs.google.com/document/d/{doc_id}/edit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_bold_basic() {
        let s = split_bold("plain **bold** more");
        assert_eq!(
            s,
            vec![
                Span { text: "plain ".into(), bold: false },
                Span { text: "bold".into(), bold: true },
                Span { text: " more".into(), bold: false },
            ]
        );
    }

    #[test]
    fn split_bold_unterminated_is_plain() {
        let s = split_bold("a **b");
        assert_eq!(s, vec![
            Span { text: "a ".into(), bold: false },
            Span { text: "**b".into(), bold: false },
        ]);
    }

    #[test]
    fn title_emits_insert_and_heading1() {
        let reqs = doc_requests(&[DocBlock::Title("Weekly Sync".into())]);
        // insertText "Weekly Sync\n" at 1, then HEADING_1 over [1, 13).
        assert_eq!(reqs[0]["insertText"]["text"], "Weekly Sync\n");
        assert_eq!(reqs[0]["insertText"]["location"]["index"], 1);
        assert_eq!(reqs[1]["updateParagraphStyle"]["paragraphStyle"]["namedStyleType"], "HEADING_1");
        assert_eq!(reqs[1]["updateParagraphStyle"]["range"]["startIndex"], 1);
        assert_eq!(reqs[1]["updateParagraphStyle"]["range"]["endIndex"], 13);
    }

    #[test]
    fn index_advances_across_blocks() {
        let reqs = doc_requests(&[
            DocBlock::Title("Hi".into()),       // "Hi\n" -> len 3, ends at 4
            DocBlock::Paragraph("there".into()), // inserts at 4
        ]);
        // First insert at 1, the paragraph insert at 4.
        assert_eq!(reqs[0]["insertText"]["location"]["index"], 1);
        // Find the second insertText.
        let second = reqs.iter().filter(|r| r.get("insertText").is_some()).nth(1).unwrap();
        assert_eq!(second["insertText"]["location"]["index"], 4);
    }

    #[test]
    fn bullet_block_emits_create_bullets() {
        let reqs = doc_requests(&[DocBlock::Bullet("do the thing".into())]);
        assert!(reqs.iter().any(|r| r.get("createParagraphBullets").is_some()));
    }

    #[test]
    fn bold_run_range_is_correct() {
        // "a **bc** d\n": bold "bc" starts after "a " (index 1+2=3) len 2.
        let reqs = doc_requests(&[DocBlock::Paragraph("a **bc** d".into())]);
        let bold = reqs.iter().find(|r| r.get("updateTextStyle").is_some()).unwrap();
        assert_eq!(bold["updateTextStyle"]["range"]["startIndex"], 3);
        assert_eq!(bold["updateTextStyle"]["range"]["endIndex"], 5);
        assert_eq!(bold["updateTextStyle"]["textStyle"]["bold"], true);
    }

    #[test]
    fn blocks_for_note_maps_bullets_and_sections() {
        let blocks = blocks_for_note(
            "My Meeting",
            "- first point\n- second point",
            "You: hello",
            true,
            true,
        );
        assert_eq!(blocks[0], DocBlock::Title("My Meeting".into()));
        assert_eq!(blocks[1], DocBlock::Heading("Summary".into()));
        assert_eq!(blocks[2], DocBlock::Bullet("first point".into()));
        assert_eq!(blocks[3], DocBlock::Bullet("second point".into()));
        assert_eq!(blocks[4], DocBlock::Heading("Transcript".into()));
        assert_eq!(blocks[5], DocBlock::Paragraph("You: hello".into()));
    }

    #[test]
    fn utf16_len_counts_astral_as_two() {
        assert_eq!(utf16_len("a"), 1);
        assert_eq!(utf16_len("\u{1F600}"), 2); // 😀
    }
}
