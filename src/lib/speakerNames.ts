// Harvest likely real names for diarized speaker labels straight from what was
// said on the call. A voiceprint is anonymous until a name is attached; this is
// the "attach a name the first time" front door.
//
// Two complementary signals, both conservative (we only ever *suggest* — the
// user confirms in the rename UI, so a wrong guess costs one dismissed chip):
//
//   1. Self-introduction — a speaker names themselves in their OWN turn:
//        "Hi, this is Hege."   "Hey, I'm Michael."   "This is Dr. Ng speaking."
//      Strong signal: the person talking told us who they are. Attributed to
//      that turn's speaker label.
//
//   2. Vocative address — one speaker names ANOTHER by addressing them:
//        "Thanks, Michael."    "Michael, what do you think?"   "Go ahead, Hege."
//      Weaker (it names someone else, not the talker), so it only breaks ties /
//      fills labels self-intros didn't cover, and only when the addressed name
//      isn't already claimed by a self-intro.
//
// Pure string logic so it can be unit-tested without audio or a model.

export type SpeakerNameGuess = {
  label: string; // the diarized label, e.g. "Speaker 1"
  name: string; // the harvested real name, e.g. "Hege"
  confidence: "high" | "medium"; // self-intro = high, vocative = medium
};

// A capitalized given name, optionally two words or an honorific:
// "Hege", "Mary Jane", "Dr. Ng". Case-SENSITIVE so it won't swallow a trailing
// lowercase word, and each word body is letters/hyphens/apostrophes only (no
// "."), so a sentence boundary like "Hege. Thanks" can't be captured as a
// two-word name. An optional leading honorific ("Dr. ", "Ms. ") is allowed.
const WORD = "[A-Z][a-z'-]{1,20}";
const HONORIFIC = "(?:(?:Dr|Mr|Mrs|Ms|Prof)\\.\\s)?";
const NAME = `(${HONORIFIC}${WORD}(?:\\s${WORD})?)`;
const HI = "[Hh][Ii]";
const HEY = "[Hh][Ee][Yy]";
const HELLO = "[Hh][Ee][Ll][Ll][Oo]";

// Words that look like names by capitalization but never are — sentence-initial
// filler, days, common capitalized openers. Guards the vocative pass especially.
const STOPWORDS = new Set([
  "I", "I'm", "Im", "Hi", "Hey", "Hello", "Yeah", "Yes", "No", "Okay", "Ok",
  "So", "Well", "Thanks", "Thank", "Sure", "Right", "Good", "Great", "Morning",
  "Afternoon", "Evening", "Today", "Tomorrow", "Yesterday", "Monday", "Tuesday",
  "Wednesday", "Thursday", "Friday", "Saturday", "Sunday", "The", "And", "But",
  "Let", "Let's", "We", "You", "They", "It", "That", "This", "Our", "My",
]);

function isStopword(n: string): boolean {
  return STOPWORDS.has(n) || STOPWORDS.has(n.split(" ")[0]);
}

// Parse the transcript into { label, text } turns. A turn is a line starting
// with "<label>: ". Mirrors extractSpeakerLabels' definition of a turn.
function turns(transcript: string): { label: string; text: string }[] {
  const out: { label: string; text: string }[] = [];
  for (const raw of transcript.split("\n")) {
    const line = raw.trimStart();
    const m = line.match(/^([^:]{1,40}):\s(.*)$/);
    if (m) out.push({ label: m[1].trim(), text: m[2].trim() });
  }
  return out;
}

// Self-intro patterns. Keyword parts are case-insensitive by character class;
// NAME stays case-sensitive so it captures exactly the name. No `i` flag.
const SELF_INTRO = [
  new RegExp(`(?:[Ii]'?m|[Ii] am)\\s+${NAME}`),
  new RegExp(`[Tt]his is\\s+${NAME}`),
  new RegExp(`[Mm]y name'?s?\\s+(?:is\\s+)?${NAME}`),
  new RegExp(`[Ii]t'?s\\s+${NAME}\\s+(?:here|speaking)\\b`),
];

// Vocative patterns: naming someone else at a turn boundary.
const VOCATIVE = [
  new RegExp(`(?:[Tt]hanks|[Tt]hank you|[Tt]hankyou),?\\s+${NAME}\\b`),
  new RegExp(`(?:${HI}|${HEY}|${HELLO})\\s+${NAME}\\b`),
  new RegExp(`^${NAME},\\s`), // "Michael, what do you think?"
  new RegExp(`(?:[Gg]o ahead|[Oo]ver to you|[Bb]ack to you),?\\s+${NAME}\\b`),
];

function cleanName(raw: string): string {
  // Trim trailing punctuation a capture might include and collapse spaces.
  return raw.replace(/[.,!?;:]+$/, "").replace(/\s+/g, " ").trim();
}

/**
 * Harvest name guesses for the diarized labels in a transcript. Returns at most
 * one guess per label, self-intros (high) winning over vocatives (medium). A
 * name already taken by one label is never also offered for another.
 */
export function harvestSpeakerNames(transcript: string): SpeakerNameGuess[] {
  if (!transcript?.trim()) return [];
  const rows = turns(transcript);
  const byLabel = new Map<string, SpeakerNameGuess>();
  const claimedNames = new Set<string>();

  // Pass 1: self-intros (high confidence). The talker names themselves.
  for (const { label, text } of rows) {
    if (byLabel.has(label)) continue;
    for (const re of SELF_INTRO) {
      const m = text.match(re);
      if (m && m[1]) {
        const name = cleanName(m[1]);
        if (!isStopword(name) && !claimedNames.has(name.toLowerCase())) {
          byLabel.set(label, { label, name, confidence: "high" });
          claimedNames.add(name.toLowerCase());
          break;
        }
      }
    }
  }

  // Pass 2: vocatives (medium). Fill labels still unnamed, with names not yet
  // claimed. A speaker addressed by name is likely a DIFFERENT speaker than the
  // talker — so we attribute the name to the most-frequent still-unnamed label
  // that isn't the current talker. Conservative: only when exactly one unnamed
  // label is a plausible target, to avoid guessing wrong on crowded calls.
  const unnamed = [...new Set(rows.map((r) => r.label))].filter((l) => !byLabel.has(l));
  for (const { label: talker, text } of rows) {
    for (const re of VOCATIVE) {
      const m = text.match(re);
      if (!m || !m[1]) continue;
      const name = cleanName(m[1]);
      if (isStopword(name) || claimedNames.has(name.toLowerCase())) continue;
      // Candidate targets: unnamed labels other than the talker.
      const targets = unnamed.filter((l) => l !== talker && !byLabel.has(l));
      if (targets.length === 1) {
        const target = targets[0];
        byLabel.set(target, { label: target, name, confidence: "medium" });
        claimedNames.add(name.toLowerCase());
      }
    }
  }

  return [...byLabel.values()];
}
