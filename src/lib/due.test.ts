import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { todayISO, daysUntil, dueState, dueLabel, byDue } from "./due";

// Pin "now" so relative math is deterministic. 2026-03-15 local noon.
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 2, 15, 12, 0, 0));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("due helpers", () => {
  it("todayISO is local YYYY-MM-DD", () => {
    expect(todayISO()).toBe("2026-03-15");
  });

  it("daysUntil counts whole days, sign-correct", () => {
    expect(daysUntil("2026-03-15")).toBe(0);
    expect(daysUntil("2026-03-16")).toBe(1);
    expect(daysUntil("2026-03-14")).toBe(-1);
    expect(daysUntil("2026-03-25")).toBe(10);
    expect(daysUntil("")).toBeNull();
    expect(daysUntil("garbage")).toBeNull();
  });

  it("dueState buckets correctly", () => {
    expect(dueState("")).toBe("none");
    expect(dueState("2026-03-10")).toBe("overdue");
    expect(dueState("2026-03-15")).toBe("today");
    expect(dueState("2026-03-17")).toBe("soon");
    expect(dueState("2026-03-25")).toBe("later");
  });

  it("dueLabel reads naturally", () => {
    expect(dueLabel("2026-03-15")).toBe("Today");
    expect(dueLabel("2026-03-16")).toBe("Tomorrow");
    expect(dueLabel("2026-03-14")).toBe("Yesterday");
    expect(dueLabel("2026-03-10")).toBe("5d overdue");
    expect(dueLabel("2026-03-18")).toBe("in 3d");
    expect(dueLabel("")).toBe("");
  });

  it("byDue sorts soonest first, undated last", () => {
    const items = [
      { due: "" },
      { due: "2026-03-25" },
      { due: "2026-03-10" },
      { due: "2026-03-16" },
    ];
    const sorted = [...items].sort(byDue).map((i) => i.due);
    expect(sorted).toEqual(["2026-03-10", "2026-03-16", "2026-03-25", ""]);
  });
});
