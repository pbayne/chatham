import { describe, expect, it } from "vitest";
import { makeNote } from "../test/fixtures";
import type { OpenActionItem } from "./ipc";
import { initials, nameHue, ownerMatches, personActivity } from "./person";

const item = (id: string, owner: string, due = "", note_id = "a"): OpenActionItem => ({
  id, note_id, text: id, owner, due, done: false, source: "auto", sort_order: 0,
  created_at: 0, updated_at: 0, note_title: "", client_id: null, client_name: null, note_created_at: 0,
});

describe("ownerMatches", () => {
  it("matches the full name or the first name, ignoring case", () => {
    expect(ownerMatches("Kari Nordvik", "Kari Nordvik")).toBe(true);
    expect(ownerMatches("kari", "Kari Nordvik")).toBe(true);
    expect(ownerMatches("Nordvik", "Kari Nordvik")).toBe(false);
    expect(ownerMatches("Karianne", "Kari Nordvik")).toBe(false);
    expect(ownerMatches("", "Kari")).toBe(false);
    expect(ownerMatches("Kari.", "Kari Nordvik")).toBe(true);
  });
});

describe("personActivity", () => {
  const notes = [
    makeNote({ id: "a", created_at: 100, client_id: "c1", transcript: "Kari Nordvik: hei\nMichael: hei" }),
    makeNote({ id: "b", created_at: 300, client_id: "c1", transcript: "Kari Nordvik: igjen" }),
    makeNote({ id: "c", created_at: 200, client_id: "c2", transcript: "Kari Nordvik: annen kunde" }),
    makeNote({ id: "d", created_at: 400, client_id: "c1", transcript: "Michael: uten henne" }),
  ];
  const clients = [
    { id: "c1", name: "Nordvik AS", created_at: 0, updated_at: 0 },
    { id: "c2", name: "Sparebanken Vest", created_at: 0, updated_at: 0 },
  ];

  it("lists their meetings newest first and the last time you met", () => {
    const a = personActivity("Kari Nordvik", notes, [], clients);
    expect(a.notes.map((n) => n.id)).toEqual(["b", "c", "a"]);
    expect(a.lastSeen).toBe(300);
  });

  it("counts the customers their meetings are filed under", () => {
    expect(personActivity("Kari Nordvik", notes, [], clients).clients).toEqual([
      { id: "c1", name: "Nordvik AS", meetings: 2 },
      { id: "c2", name: "Sparebanken Vest", meetings: 1 },
    ]);
  });

  it("keeps only their open items, soonest due first", () => {
    const open = [item("x", "Michael"), item("late", "Kari", "2099-01-02"), item("soon", "kari nordvik", "2099-01-01")];
    expect(personActivity("Kari Nordvik", notes, open, clients).commitments.map((i) => i.id)).toEqual(["soon", "late"]);
  });

  it("counts a first-name-only owner only from a meeting they spoke in", () => {
    const open = [item("mine", "Kari", "", "a"), item("other-kari", "Kari", "", "d"), item("full", "Kari Nordvik", "", "d")];
    expect(personActivity("Kari Nordvik", notes, open, clients).commitments.map((i) => i.id).sort()).toEqual(["full", "mine"]);
  });

  it("is empty for someone who never spoke", () => {
    const a = personActivity("Nobody", notes, [], clients);
    expect(a).toEqual({ notes: [], commitments: [], lastSeen: null, clients: [] });
  });
});

describe("initials / nameHue", () => {
  it("takes the first and last word's first letter", () => {
    expect(initials("Kari Nordvik")).toBe("KN");
    expect(initials("Hege")).toBe("H");
    expect(initials("  ola  van  berg ")).toBe("OB");
    expect(initials("Åse Øien")).toBe("ÅØ");
    expect(initials("")).toBe("?");
  });
  it("gives a name the same bucket every time, in range", () => {
    expect(nameHue("Kari Nordvik")).toBe(nameHue("kari nordvik "));
    for (const n of ["Michael", "Hege", "Ola Berg", "Anne Lie"]) expect(nameHue(n)).toBeLessThan(4);
  });
});
