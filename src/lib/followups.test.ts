import { describe, expect, it } from "vitest";
import { todayISO } from "./due";
import { NO_OWNER, filterFollowups, followupOwners } from "./followups";

const day = (n: number) => {
  const d = new Date();
  d.setDate(d.getDate() + n);
  return todayISO(d);
};
const items = [
  { id: "late", text: "a", owner: "Kari", due: day(-2) },
  { id: "today", text: "b", owner: "kari ", due: day(0) },
  { id: "five", text: "c", owner: "Ola", due: day(5) },
  { id: "far", text: "d", owner: "Ola", due: day(30) },
  { id: "none", text: "e", owner: "", due: "" },
];
const ids = (xs: { id: string }[]) => xs.map((x) => x.id);

describe("filterFollowups", () => {
  it("passes everything with no filter", () => {
    expect(ids(filterFollowups(items, "", "all"))).toEqual(["late", "today", "five", "far", "none"]);
  });
  it("matches owner case- and space-insensitively", () => {
    expect(ids(filterFollowups(items, "kari", "all"))).toEqual(["late", "today"]);
  });
  it("selects unassigned items", () => {
    expect(ids(filterFollowups(items, NO_OWNER, "all"))).toEqual(["none"]);
  });
  it("filters by due window", () => {
    expect(ids(filterFollowups(items, "", "overdue"))).toEqual(["late"]);
    expect(ids(filterFollowups(items, "", "week"))).toEqual(["late", "today", "five"]);
    expect(ids(filterFollowups(items, "", "undated"))).toEqual(["none"]);
  });
  it("combines both axes", () => {
    expect(ids(filterFollowups(items, "ola", "week"))).toEqual(["five"]);
  });
});

describe("followupOwners", () => {
  it("dedupes case-insensitively, most items first, skips blanks", () => {
    expect(followupOwners(items)).toEqual([
      { value: "kari", label: "Kari", count: 2 },
      { value: "ola", label: "Ola", count: 2 },
    ]);
  });
});
