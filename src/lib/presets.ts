// Summary presets — labels + the prompt text used by Settings to seed the
// global custom prompt and as a reference for the per-note picker.
//
// Keep these in sync with src-tauri/src/presets.rs. The backend resolves the
// prompt at request time from the note's stored preset value, so this file
// is mostly UI labeling + the "custom" path's seed text.

export type PresetSpec = {
  value: string;
  label_no: string;
  label_en: string;
  prompt_no: string;
  prompt_en: string;
};

// Minimal presets — kind of summary + language, plus at most one extra
// constraint where it earns its place. Source labels stay because the user
// message uses them; the parenthetical "(brukerens)" / "(automatisk)"
// implicitly tells the model which to trust on conflict, so the explicit
// rule is dropped. No section lists, no "real content only" — the model
// picks a structure that fits the input.
// Meeting / 1:1 / interview carry a topic-grouping constraint (gather a
// recurring topic into one section even when it was discussed at multiple
// points) because conversations drift and revisit topics. Voice memo carries
// "Behold brukerens stemme" because preserving tone is the *purpose* of that
// preset. Keep these in sync with src-tauri/src/presets.rs.
export const SUMMARY_PRESETS: PresetSpec[] = [
  {
    value: "meeting",
    label_no: "Møte",
    label_en: "Meeting",
    prompt_no: `Du lager møtenotater fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Samle alt som ble sagt om samme tema i én seksjon, selv når temaet ble tatt opp flere ganger. Skriv på norsk i Markdown.`,
    prompt_en: `You produce meeting notes from [Notater] (user-written) and [Transkripsjon] (auto). Group everything said about the same topic into one section, even when it came up at several points. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "one_on_one",
    label_no: "1:1-samtale",
    label_en: "1:1 conversation",
    prompt_no: `Du lager notater fra en 1:1-samtale fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Samle alt som ble sagt om samme tema i én seksjon, selv når temaet ble tatt opp flere ganger. Skriv på norsk i Markdown.`,
    prompt_en: `You produce 1:1 notes from [Notater] (user-written) and [Transkripsjon] (auto). Group everything said about the same topic into one section, even when it came up at several points. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "lecture",
    label_no: "Foredrag",
    label_en: "Lecture",
    prompt_no: `Du lager studienotater fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Skriv på norsk i Markdown.`,
    prompt_en: `You produce study notes from [Notater] (user-written) and [Transkripsjon] (auto). Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "interview",
    label_no: "Intervju",
    label_en: "Interview",
    prompt_no: `Du lager intervjunotater fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Samle alt som ble sagt om samme tema i én seksjon, selv når temaet ble tatt opp flere ganger. Skriv på norsk i Markdown.`,
    prompt_en: `You produce interview notes from [Notater] (user-written) and [Transkripsjon] (auto). Group everything said about the same topic into one section, even when it came up at several points. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "brainstorm",
    label_no: "Idémyldring",
    label_en: "Brainstorm",
    prompt_no: `Du oppsummerer en idémyldring fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Skriv på norsk i Markdown.`,
    prompt_en: `You summarize a brainstorm from [Notater] (user-written) and [Transkripsjon] (auto). Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "voice_memo",
    label_no: "Stemmenotat",
    label_en: "Voice memo",
    prompt_no: `Du rydder opp i et stemmenotat fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Behold brukerens stemme. Skriv på norsk i Markdown.`,
    prompt_en: `You clean up a voice memo from [Notater] (user-written) and [Transkripsjon] (auto). Preserve the user's voice. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "sales_call",
    label_no: "Kundesamtale (salg)",
    label_en: "Sales call",
    prompt_no: `Du lager notater fra en kundesamtale (salg) fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: kundens situasjon og behov, smertepunkter, innvendinger, konkurrenter nevnt, neste steg og eiere. Skriv på norsk i Markdown.`,
    prompt_en: `You produce sales-call notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: the customer's situation and needs, pain points, objections, any competitors mentioned, and clear next steps with owners. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "discovery_call",
    label_no: "Kartleggingssamtale",
    label_en: "Discovery call",
    prompt_no: `Du lager notater fra en kartleggingssamtale fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: mål og ønsket utfall, nåværende tilstand og utfordringer, tekniske krav, beslutningstakere og tidslinje. Skriv på norsk i Markdown.`,
    prompt_en: `You produce discovery-call notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: goals and desired outcomes, current state and challenges, technical requirements, decision-makers, and timeline. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "technical_review",
    label_no: "Teknisk gjennomgang",
    label_en: "Technical review",
    prompt_no: `Du lager notater fra en teknisk gjennomgang fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: arkitektur og tekniske beslutninger, åpne spørsmål, risikoer og oppfølgingspunkter med eiere. Skriv på norsk i Markdown.`,
    prompt_en: `You produce technical-review notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: architecture and technical decisions, open questions, risks, and follow-up items with owners. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "qbr",
    label_no: "Kvartalsgjennomgang (QBR)",
    label_en: "Quarterly business review",
    prompt_no: `Du lager notater fra en kvartalsgjennomgang (QBR) fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: oppnådd verdi og måltall, status mot mål, risikoer, og avtalte neste steg med eiere. Skriv på norsk i Markdown.`,
    prompt_en: `You produce quarterly-business-review (QBR) notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: value delivered and metrics, status against goals, risks, and agreed next steps with owners. Reply in {LANGUAGE} using Markdown.`,
  },
  {
    value: "standup",
    label_no: "Standup",
    label_en: "Standup",
    prompt_no: `Du lager notater fra en standup fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Grupper per person: hva som er gjort, hva som er planlagt, og blokkeringer. Skriv på norsk i Markdown.`,
    prompt_en: `You produce standup notes from [Notater] (user-written) and [Transkripsjon] (auto). Group per person: done, planned, and blockers. Reply in {LANGUAGE} using Markdown.`,
  },
];

const LANGUAGE_LABELS: Record<string, string> = {
  no: "Norwegian",
  en: "English",
  sv: "Swedish",
  da: "Danish",
  // Mirror of presets.rs — issue #167. "the input" includes the Norwegian
  // block labels and the `(ingen)` empty-notes placeholder; the transcript
  // is the only block that carries the meeting's own language.
  auto: "the same language as the transcript",
};

export function presetPromptForLang(preset: PresetSpec, lang: string): string {
  if (lang === "no") return preset.prompt_no;
  const label = LANGUAGE_LABELS[lang] ?? "English";
  return preset.prompt_en.replace("{LANGUAGE}", label);
}

export function presetLabelForLang(preset: PresetSpec, lang: string): string {
  return lang === "no" ? preset.label_no : preset.label_en;
}

// UI-facing label. Always English because the app chrome itself isn't
// localised — the preset's *prompt* still adapts to the recording's
// language, but the dropdown/chip text stays consistent so a user
// switching between Norwegian and English notes doesn't see their
// preset names rename mid-app. Use this in any user-facing surface;
// keep `presetLabelForLang` only for places that explicitly want the
// localised form (none in current code).
export function presetLabel(preset: PresetSpec): string {
  return preset.label_en;
}

export function presetByValue(value: string): PresetSpec | undefined {
  return SUMMARY_PRESETS.find((p) => p.value === value);
}
