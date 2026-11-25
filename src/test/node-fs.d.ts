// An ambient stand-in for the `node:fs` functions the theme tests use
// (themes.contract.test.ts reads the theme CSS as text; tokens.usage.test.ts
// also walks src/ to check every var(--token) a component reads is declared).
//
// Deliberately NOT @types/node. The app's tsconfig covers `src`, so installing
// node types would put `process` and `Buffer` in scope for frontend code that
// must never touch them, and — the concrete hazard — would add node's
// `setTimeout` overload, flipping its return type from `number` to
// `NodeJS.Timeout` across every component that stores a timer id.
//
// Reading the CSS through Vite instead was tried first and doesn't work: both
// `import css from "./warm.css?raw"` and the `import.meta.glob` equivalent
// resolve to an empty string under vitest, because CSS modules are stubbed in
// the test environment before the `?raw` query is honoured.
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function readdirSync(path: string): string[];
  export function statSync(path: string): { isDirectory(): boolean };
}

// Vitest defines this in test files. `import.meta.url` is NOT an alternative
// here: under the jsdom environment it resolves to an http:// URL (jsdom's
// document base), which node:fs rejects with "The URL must be of scheme file".
declare const __dirname: string;
