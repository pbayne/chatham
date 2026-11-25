import { SPEAKER_COLORS } from "./SpeakerLabels";
import { initials, nameHue } from "../lib/person";

/** Initials on a soft tint of the person's speaker colour. The colour is keyed
    on the name (nameHue), so a person wears the same tint on People, Person and
    Customer — a transcript's per-note order of appearance can't give that. */
export function Avatar({ name, size = 32 }: { name: string; size?: number }) {
  const color = SPEAKER_COLORS[nameHue(name, SPEAKER_COLORS.length)];
  return (
    <span
      aria-hidden
      className="shrink-0 rounded-full flex items-center justify-center font-semibold tabular-nums select-none"
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.38),
        // The raw speaker colour is a fill colour — gold on its own tint reads
        // 2.4:1 in warm light. Mixing it 55% toward the display ink keeps the
        // hue and clears 5.1:1 in every palette and mode (computed from the
        // theme files), where the initials are 9–12px text needing 4.5:1.
        color: `color-mix(in srgb, ${color} 55%, var(--color-text-display))`,
        background: `color-mix(in srgb, ${color} 16%, var(--color-surface))`,
      }}
    >
      {initials(name)}
    </span>
  );
}
