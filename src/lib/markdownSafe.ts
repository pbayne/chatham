// Markdown-input sanitation (#inbox-triage / email summaries).
//
// ReactMarkdown parses CommonMark, which recognizes raw-HTML *tag syntax* even
// though we never render raw HTML. The practical fallout: text copied from
// email threads contains angle-bracket segments that are NOT markdown —
// `<joanna.zouhouradi@databricks.com>`, `On Tue … <someone> wrote:`, generic
// `<placeholder>` — and the parser treats each `<…>` as an HTML tag and drops
// it from the output. A stray unclosed `<` is worse: it swallows everything up
// to the next `>` (e.g. "I'll ma<…> wrote" renders as "I'll ma wrote"), so whole
// words vanish. That showed up as "characters not showing up" in summaries of
// email follow-up threads.
//
// Fix: before parsing, escape the `<`/`>` of any angle-bracket run that is NOT
// a legitimate markdown autolink (a real URL/scheme or a bare email per GFM).
// CommonMark autolinks require an absolute URI with a scheme (`<https://…>`,
// `<mailto:…>`) or, under GFM, a plain email `<a@b.com>`. Everything else —
// addresses with display noise, `<word>`, `<a.b.c@d>` that GFM won't autolink,
// unmatched `<` — becomes literal text via HTML entities, so it renders exactly
// as written. We deliberately do NOT touch fenced/inline code or real tags we
// might add later; summaries are model-generated markdown prose, not HTML.

// A conservative "this is a real autolink, leave it alone" test. Matches only
// the CommonMark absolute-URI autolink (`<https://…>`, `<mailto:…>`). Bare
// emails in angle brackets are deliberately NOT treated as autolinks: in
// meeting-notes prose `<name@host>` should render verbatim (brackets and all),
// not silently become a mailto link or — for dotted locals react-markdown
// won't autolink — get dropped as an unknown HTML tag.
const AUTOLINK = /^<[a-zA-Z][a-zA-Z0-9+.-]*:[^<>\s]+>$/;

// Escape a single `<` or `>` to its HTML entity. remark renders entities as the
// literal character, so the reader sees `<x>` not a dropped tag.
function esc(ch: string): string {
  return ch === "<" ? "&lt;" : "&gt;";
}

/**
 * Neutralize angle brackets that CommonMark would misread as HTML tags, while
 * preserving genuine autolinks and the contents of code spans/blocks.
 *
 * Strategy: walk the source, skipping fenced code (``` … ```) and inline code
 * (`` `…` ``) untouched. Outside code, find each `<…>` run; if it's a real
 * autolink, keep it; otherwise escape its brackets. Any `<` with no matching
 * `>` on the way is escaped too (that's the word-eating case).
 */
export function sanitizeMarkdownAngles(src: string): string {
  if (!src || src.indexOf("<") === -1) return src;

  let out = "";
  let i = 0;

  // Track fenced code blocks (``` or ~~~) and inline code spans so we never
  // rewrite anything a reader expects verbatim.
  let inFence = false;
  let fenceMarker = "";

  const lines = src.split("\n");
  for (let li = 0; li < lines.length; li++) {
    const line = lines[li];
    const fenceMatch = line.match(/^(\s*)(`{3,}|~{3,})/);
    if (fenceMatch) {
      const marker = fenceMatch[2][0];
      if (!inFence) {
        inFence = true;
        fenceMarker = marker;
      } else if (marker === fenceMarker) {
        inFence = false;
        fenceMarker = "";
      }
      out += (li > 0 ? "\n" : "") + line;
      continue;
    }
    if (inFence) {
      out += (li > 0 ? "\n" : "") + line;
      continue;
    }

    // Non-fenced line: process char by char, respecting inline `code`.
    let lineOut = "";
    i = 0;
    const len = line.length;
    let inCode = false;
    let codeTicks = 0;
    while (i < len) {
      const ch = line[i];
      if (ch === "`") {
        // Count the run of backticks (inline code delimiter length matters).
        let run = 0;
        while (i + run < len && line[i + run] === "`") run++;
        if (!inCode) {
          inCode = true;
          codeTicks = run;
        } else if (run === codeTicks) {
          inCode = false;
          codeTicks = 0;
        }
        lineOut += "`".repeat(run);
        i += run;
        continue;
      }
      if (inCode) {
        lineOut += ch;
        i++;
        continue;
      }
      if (ch === "<") {
        const close = line.indexOf(">", i + 1);
        if (close !== -1) {
          const run = line.slice(i, close + 1); // includes < … >
          if (AUTOLINK.test(run)) {
            lineOut += run; // genuine autolink: keep as-is
          } else {
            lineOut += esc("<") + run.slice(1, -1) + esc(">");
          }
          i = close + 1;
          continue;
        }
        // Unmatched '<' — the word-eating case. Escape it.
        lineOut += esc("<");
        i++;
        continue;
      }
      if (ch === ">") {
        // A stray '>' at the very start of a line is a blockquote; keep it.
        // Elsewhere it's harmless, but escaping keeps symmetry with escaped '<'.
        lineOut += ch === ">" && lineOut.trim() === "" ? ">" : ch;
        i++;
        continue;
      }
      lineOut += ch;
      i++;
    }
    out += (li > 0 ? "\n" : "") + lineOut;
  }

  return out;
}
