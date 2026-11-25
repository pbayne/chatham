// Every `var(--token)` a page or component reads must be a token some theme
// declares. An undefined custom property renders as nothing — a hover that never
// tints, a border radius of 0 — and no test or typecheck notices, because the
// class string is valid. This reads the sources as text and names the strays.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { describe, expect, it } from "vitest";

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = `${dir}/${name}`;
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

const SRC = `${__dirname}/../..`;
const css = walk(`${SRC}/styles`).filter((p) => p.endsWith(".css")).map((p) => readFileSync(p, "utf8")).join("\n");
const declared = new Set([...css.matchAll(/(--[a-z0-9-]+)\s*:/g)].map((m) => m[1]));
// Set inline by components at runtime (style={{ "--x": ... }}), not by a theme.
// Set by Radix on its content, or read with an inline fallback (`var(--x, 2px)`).
const RUNTIME = /^--(radix-|switch-inset$)/;

describe("design tokens", () => {
  it("every var(--token) in src/pages and src/components is declared by a stylesheet", () => {
    const files = [...walk(`${SRC}/pages`), ...walk(`${SRC}/components`)].filter(
      (p) => /\.tsx?$/.test(p) && !/\.test\.tsx?$/.test(p),
    );
    const stray: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(/var\((--[a-z0-9-]+)/g)) {
        const tok = m[1];
        if (declared.has(tok) || RUNTIME.test(tok)) continue;
        stray.push(`${f.slice(SRC.length + 1)}: ${tok}`);
      }
    }
    expect([...new Set(stray)]).toEqual([]);
  });
});
