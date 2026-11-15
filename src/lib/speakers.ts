// Speaker-label helpers for transcripts. Pure string functions, extracted
// from Note.tsx so they can be unit-tested in isolation (see speakers.test.ts).
// A "speaker turn" is any line starting with "<label>: " where the label is
// non-colon text. These back the speaker-rename UI, so the regex behaviour is
// load-bearing — hence the dedicated tests.

// Parse the transcript for speaker turn prefixes — any line starting with
// `<label>: ` (label can be any non-colon text) is treated as a speaker
// turn. Returns labels in first-encounter order, deduplicated.
export function extractSpeakerLabels(transcript: string): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const rawLine of transcript.split("\n")) {
    const line = rawLine.trimStart();
    const match = line.match(/^([^:]{1,40}):\s/);
    if (match) {
      const label = match[1].trim();
      if (!seen.has(label)) {
        seen.add(label);
        result.push(label);
      }
    }
  }
  return result;
}

// The transcript without its label prefixes — the words, not who said them.
export function stripSpeakerLabels(transcript: string): string {
  return transcript.replace(/^(\s*)[^:\n]{1,40}:\s/gm, "$1");
}

// Rewrite the transcript so every "<oldLabel>: " line start becomes
// "<newLabel>: ". Anchored to line starts via a multi-line regex; bare
// occurrences of the label inside text are left alone. Escapes regex
// metacharacters in oldLabel so renaming to/from values like "Speaker 1?"
// doesn't break.
export function renameSpeakerInTranscript(transcript: string, oldLabel: string, newLabel: string): string {
  if (oldLabel === newLabel) return transcript;
  const escaped = oldLabel.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`^(\\s*)${escaped}: `, "gm");
  return transcript.replace(re, `$1${newLabel}: `);
}

// Whether a speaker label is a real, named person worth tracking in the People
// directory — as opposed to a generic diarizer placeholder ("Speaker 1",
// "Speaker 2"), the user themselves ("You", "Me"), or a system label. Named
// people are what the directory organizes; placeholders are noise until the
// user renames them.
export function isNamedPerson(label: string): boolean {
  const l = label.trim();
  if (!l) return false;
  // Generic diarizer placeholders: "Speaker 1", "Speaker", "Talker 2", etc.
  if (/^(speaker|talker|unknown|participant|person)\b/i.test(l)) return false;
  // The user's own first-person labels.
  if (/^(you|me|myself)$/i.test(l)) return false;
  // Pure numbers / single chars aren't names.
  if (l.length < 2) return false;
  if (/^\d+$/.test(l)) return false;
  return true;
}

