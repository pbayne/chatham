import { describe, it, expect } from "vitest";
import { harvestSpeakerNames } from "./speakerNames";

describe("harvestSpeakerNames", () => {
  it("returns nothing for empty or label-less input", () => {
    expect(harvestSpeakerNames("")).toEqual([]);
    expect(harvestSpeakerNames("just some prose with no turns")).toEqual([]);
  });

  it("harvests a self-introduction (high confidence)", () => {
    const t = "Speaker 1: Hi, I'm Hege and I lead data.\nSpeaker 2: Great to meet you.";
    const g = harvestSpeakerNames(t);
    expect(g).toContainEqual({ label: "Speaker 1", name: "Hege", confidence: "high" });
  });

  it("handles 'this is X' and 'my name is X'", () => {
    expect(harvestSpeakerNames("Speaker 1: This is Michael speaking.")).toContainEqual({
      label: "Speaker 1",
      name: "Michael",
      confidence: "high",
    });
    expect(harvestSpeakerNames("Speaker 2: My name is Sarah.")).toContainEqual({
      label: "Speaker 2",
      name: "Sarah",
      confidence: "high",
    });
  });

  it("captures two-word names and titles", () => {
    expect(harvestSpeakerNames("Speaker 1: Hi, I'm Mary Jane.")).toContainEqual({
      label: "Speaker 1",
      name: "Mary Jane",
      confidence: "high",
    });
  });

  it("harvests a vocative when exactly one other label is unnamed (medium)", () => {
    // Speaker 1 introduces self (Hege); addresses Michael → the one other
    // unnamed label (Speaker 2) gets Michael.
    const t =
      "Speaker 1: Hi, I'm Hege. Thanks, Michael, for joining.\nSpeaker 2: Happy to be here.";
    const g = harvestSpeakerNames(t);
    expect(g).toContainEqual({ label: "Speaker 1", name: "Hege", confidence: "high" });
    expect(g).toContainEqual({ label: "Speaker 2", name: "Michael", confidence: "medium" });
  });

  it("does not assign a vocative when the target is ambiguous (3+ labels)", () => {
    const t =
      "Speaker 1: Thanks, Michael.\nSpeaker 2: Sure.\nSpeaker 3: Okay.";
    const g = harvestSpeakerNames(t);
    // Two unnamed labels besides the talker → ambiguous → no guess.
    expect(g.find((x) => x.name === "Michael")).toBeUndefined();
  });

  it("never offers the same name for two labels", () => {
    const t = "Speaker 1: I'm Hege.\nSpeaker 2: I'm Hege too, confusingly.";
    const g = harvestSpeakerNames(t);
    const heges = g.filter((x) => x.name === "Hege");
    expect(heges).toHaveLength(1);
  });

  it("ignores stopwords that look like names", () => {
    const t = "Speaker 1: Thanks, Okay.\nSpeaker 2: Hi, I'm Yeah.";
    // "Okay" and "Yeah" are stopwords — nothing harvested.
    expect(harvestSpeakerNames(t)).toEqual([]);
  });

  it("self-intro wins over a later vocative for the same label", () => {
    const t = "Speaker 1: I'm Hege.\nSpeaker 2: Thanks, Hege.";
    const g = harvestSpeakerNames(t);
    const s1 = g.find((x) => x.label === "Speaker 1");
    expect(s1).toEqual({ label: "Speaker 1", name: "Hege", confidence: "high" });
  });

  it("leading-vocative pattern 'Michael, ...' works", () => {
    const t = "Speaker 1: I'm Hege.\nSpeaker 1: Michael, what do you think?\nSpeaker 2: Good question.";
    const g = harvestSpeakerNames(t);
    expect(g).toContainEqual({ label: "Speaker 2", name: "Michael", confidence: "medium" });
  });
});
