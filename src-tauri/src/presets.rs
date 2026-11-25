// Summary presets, mirrored from the frontend so the backend can resolve a
// preset → prompt without round-tripping. Norwegian and English versions per
// preset; English uses {LANGUAGE} which we substitute at request time.
//
// Keep these in sync with src/lib/presets.ts::SUMMARY_PRESETS.

use crate::languages;

/// The built-in default preset. What an unknown or missing preset value
/// resolves to, and what the `custom` fallbacks in `resolve_prompt` use
/// when the user's own prompt row isn't available. Pinned to `ALL[0]` by
/// `the_default_preset_is_the_first_entry`.
pub const DEFAULT_SUMMARY_PRESET: &str = "meeting";

/// What transcribed Feeds episodes are summarised as.
pub const PODCAST_PRESET: &str = "podcast";

pub fn prompt(preset: &str, lang: &str) -> String {
    let entry = ALL.iter().find(|p| p.value == preset).unwrap_or(&ALL[0]);
    if lang == "no" {
        entry.no.to_string()
    } else {
        // Issue #167: "the input" includes this prompt's own Norwegian
        // labels and, when the user typed nothing, the Norwegian `(ingen)`
        // placeholder — so on `auto` the only language cue in reach was
        // Norwegian. The transcript is the one block that carries the
        // meeting's actual language.
        let label = if lang == "auto" {
            "the same language as the transcript"
        } else {
            languages::english_name(lang)
        };
        entry.en.replace("{LANGUAGE}", label)
    }
}

struct Preset {
    value: &'static str,
    no: &'static str,
    en: &'static str,
}

/// English display label for a preset value — the report's title in the
/// switcher. Mirrors the `label_en` values in `src/lib/presets.ts`. Falls back
/// to a title-cased form of the value for a custom/unknown preset.
pub fn label_for(preset: &str) -> String {
    match preset {
        "meeting" => "Meeting",
        "one_on_one" => "1:1 conversation",
        "lecture" => "Lecture",
        "interview" => "Interview",
        "brainstorm" => "Brainstorm",
        "voice_memo" => "Voice memo",
        "sales_call" => "Sales call",
        "discovery_call" => "Discovery call",
        "technical_review" => "Technical review",
        "qbr" => "Quarterly business review",
        "standup" => "Standup",
        "podcast" => "Podcast episode",
        other => {
            // custom:<id> or an unknown value → a readable fallback.
            let base = other.strip_prefix("custom:").unwrap_or(other);
            return title_case(base);
        }
    }
    .to_string()
}

fn title_case(s: &str) -> String {
    let cleaned = s.replace(['_', '-'], " ");
    let mut out = String::new();
    for (i, word) in cleaned.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "Report".to_string()
    } else {
        out
    }
}

// Minimal presets — kind of summary + language, plus at most one extra
// constraint where it earns its place. Source labels stay because the user
// message uses them; the parenthetical "(brukerens)" / "(automatisk)"
// implicitly tells the model which to trust on conflict, so we drop the
// explicit rule. No section list, no "real content only" — let the model
// pick a structure that fits the input.
// Meeting / 1:1 / interview carry a topic-grouping constraint (gather a
// recurring topic into one section even when it was discussed at multiple
// points) because conversations drift and revisit topics. Voice memo carries
// "Behold brukerens stemme" because preserving tone is the *purpose* of that
// preset.
const ALL: &[Preset] = &[
    Preset {
        value: "meeting",
        no: "Du lager møtenotater fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Samle alt som ble sagt om samme tema i én seksjon, selv når temaet ble tatt opp flere ganger. Skriv på norsk i Markdown.",
        en: "You produce meeting notes from [Notater] (user-written) and [Transkripsjon] (auto). Group everything said about the same topic into one section, even when it came up at several points. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "one_on_one",
        no: "Du lager notater fra en 1:1-samtale fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Samle alt som ble sagt om samme tema i én seksjon, selv når temaet ble tatt opp flere ganger. Skriv på norsk i Markdown.",
        en: "You produce 1:1 notes from [Notater] (user-written) and [Transkripsjon] (auto). Group everything said about the same topic into one section, even when it came up at several points. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "lecture",
        no: "Du lager studienotater fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Skriv på norsk i Markdown.",
        en: "You produce study notes from [Notater] (user-written) and [Transkripsjon] (auto). Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "interview",
        no: "Du lager intervjunotater fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Samle alt som ble sagt om samme tema i én seksjon, selv når temaet ble tatt opp flere ganger. Skriv på norsk i Markdown.",
        en: "You produce interview notes from [Notater] (user-written) and [Transkripsjon] (auto). Group everything said about the same topic into one section, even when it came up at several points. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "brainstorm",
        no: "Du oppsummerer en idémyldring fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Skriv på norsk i Markdown.",
        en: "You summarize a brainstorm from [Notater] (user-written) and [Transkripsjon] (auto). Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "voice_memo",
        no: "Du rydder opp i et stemmenotat fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Behold brukerens stemme. Skriv på norsk i Markdown.",
        en: "You clean up a voice memo from [Notater] (user-written) and [Transkripsjon] (auto). Preserve the user's voice. Reply in {LANGUAGE} using Markdown.",
    },
    // Sales / customer call types (added for customer-facing GTM work). Each
    // asks for the structure that call type actually needs — action items,
    // next steps, risks — rather than a generic recap.
    Preset {
        value: "sales_call",
        no: "Du lager notater fra en kundesamtale (salg) fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: kundens situasjon og behov, smertepunkter, innvendinger, konkurrenter nevnt, neste steg og eiere. Skriv på norsk i Markdown.",
        en: "You produce sales-call notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: the customer's situation and needs, pain points, objections, any competitors mentioned, and clear next steps with owners. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "discovery_call",
        no: "Du lager notater fra en kartleggingssamtale fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: mål og ønsket utfall, nåværende tilstand og utfordringer, tekniske krav, beslutningstakere og tidslinje. Skriv på norsk i Markdown.",
        en: "You produce discovery-call notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: goals and desired outcomes, current state and challenges, technical requirements, decision-makers, and timeline. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "technical_review",
        no: "Du lager notater fra en teknisk gjennomgang fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: arkitektur og tekniske beslutninger, åpne spørsmål, risikoer og oppfølgingspunkter med eiere. Skriv på norsk i Markdown.",
        en: "You produce technical-review notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: architecture and technical decisions, open questions, risks, and follow-up items with owners. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "qbr",
        no: "Du lager notater fra en kvartalsgjennomgang (QBR) fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Ta med: oppnådd verdi og måltall, status mot mål, risikoer, og avtalte neste steg med eiere. Skriv på norsk i Markdown.",
        en: "You produce quarterly-business-review (QBR) notes from [Notater] (user-written) and [Transkripsjon] (auto). Capture: value delivered and metrics, status against goals, risks, and agreed next steps with owners. Reply in {LANGUAGE} using Markdown.",
    },
    Preset {
        value: "standup",
        no: "Du lager notater fra en standup fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Grupper per person: hva som er gjort, hva som er planlagt, og blokkeringer. Skriv på norsk i Markdown.",
        en: "You produce standup notes from [Notater] (user-written) and [Transkripsjon] (auto). Group per person: done, planned, and blockers. Reply in {LANGUAGE} using Markdown.",
    },    // Transcribed Feeds episodes (commands/feeds.rs) are filed with this.
    Preset {
        value: "podcast",
        no: "Du oppsummerer en podkastepisode fra [Notater] (brukerens) og [Transkripsjon] (automatisk). Start med to eller tre setninger om hva episoden handler om, deretter hovedpoengene og lærdommene gruppert etter tema, og til slutt verdt å merke seg: sitater, personer, produkter eller ressurser som ble nevnt. Skriv på norsk i Markdown.",
        en: "You summarize a podcast episode from [Notater] (user-written) and [Transkripsjon] (auto). Open with two or three sentences on what the episode is about, then the key points and takeaways grouped by topic, then any notable quotes, people, products or resources mentioned. Reply in {LANGUAGE} using Markdown.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #167 — "the input" is ambiguous when the notes are empty, and
    // the Norwegian block labels are part of "the input" too. Name the one
    // block that actually carries the meeting's language.
    #[test]
    fn auto_points_every_preset_at_the_transcript() {
        for p in ALL {
            let out = prompt(p.value, "auto");
            assert!(
                out.contains("the same language as the transcript"),
                "{}: {out}",
                p.value
            );
        }
    }

    // `prompt` falls back to `ALL[0]` for an unrecognised preset, so the
    // named default has to be that same entry or the two disagree about
    // what "the built-in default" means.
    #[test]
    fn the_default_preset_is_the_first_entry() {
        assert_eq!(ALL[0].value, DEFAULT_SUMMARY_PRESET);
        assert_eq!(
            prompt("no-such-preset", "en"),
            prompt(DEFAULT_SUMMARY_PRESET, "en")
        );
    }

    #[test]
    fn explicit_language_substitutes_its_english_name() {
        assert!(prompt("meeting", "en").contains("Reply in English"));
        assert!(prompt("meeting", "no").contains("Skriv på norsk"));
    }
}
