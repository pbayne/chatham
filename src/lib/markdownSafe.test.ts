import { describe, it, expect } from "vitest";
import { sanitizeMarkdownAngles } from "./markdownSafe";

describe("sanitizeMarkdownAngles", () => {
  it("escapes an email address in angle brackets so it isn't eaten as a tag", () => {
    const out = sanitizeMarkdownAngles(
      "On Tue, Joanna <joanna.zouhouradi@databricks.com> wrote:",
    );
    // A dotted-local email GFM won't autolink -> must be escaped to render.
    expect(out).toContain("&lt;joanna.zouhouradi@databricks.com&gt;");
    expect(out).toContain("wrote:");
  });

  it("recovers the word-eating case: an unmatched '<' no longer swallows text", () => {
    const out = sanitizeMarkdownAngles("Thanks, I'll ma<ke it work");
    expect(out).toContain("Thanks, I'll ma&lt;ke it work");
  });

  it("escapes a generic <placeholder> that is not a real tag", () => {
    expect(sanitizeMarkdownAngles("replace <name> here")).toBe(
      "replace &lt;name&gt; here",
    );
  });

  it("preserves a genuine URL autolink", () => {
    const s = "see <https://example.com/x> for more";
    expect(sanitizeMarkdownAngles(s)).toBe(s);
  });

  it("escapes a bare email in angle brackets so it renders verbatim", () => {
    // In notes prose `<email>` should display as written, not become a link or
    // get dropped. Brackets are escaped; the address is preserved.
    const out = sanitizeMarkdownAngles("mail <bob@example.com> now");
    expect(out).toBe("mail &lt;bob@example.com&gt; now");
  });

  it("preserves a mailto: autolink", () => {
    const s = "<mailto:bob@example.com>";
    expect(sanitizeMarkdownAngles(s)).toBe(s);
  });

  it("leaves text with no angle brackets untouched", () => {
    const s = "# Heading\n\nNormal **bold** text.";
    expect(sanitizeMarkdownAngles(s)).toBe(s);
  });

  it("does not touch angle brackets inside inline code", () => {
    const s = "use `<div>` in HTML";
    expect(sanitizeMarkdownAngles(s)).toBe(s);
  });

  it("does not touch angle brackets inside a fenced code block", () => {
    const s = "```\n<not a tag> and <x@y.z>\n```";
    expect(sanitizeMarkdownAngles(s)).toBe(s);
  });

  it("keeps a leading blockquote '>' as markdown", () => {
    const s = "> quoted line";
    expect(sanitizeMarkdownAngles(s)).toBe(s);
  });

  it("handles multiple brackets on one line", () => {
    const out = sanitizeMarkdownAngles("<a> and <b@c.d.e> and <https://x.io>");
    expect(out).toContain("&lt;a&gt;");
    expect(out).toContain("<https://x.io>"); // real autolink preserved
  });

  it("is a no-op on empty/plain input", () => {
    expect(sanitizeMarkdownAngles("")).toBe("");
    expect(sanitizeMarkdownAngles("hello")).toBe("hello");
  });
});
