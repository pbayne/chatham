import { useThemeStore } from "../../../lib/theme";
import { PALETTE_REGISTRY, usePaletteStore } from "../../../lib/palette";
import { useTimeFormatStore, type TimeFormat } from "../../../lib/timeFormat";
import { Row, Section } from "../components/Section";
import { Segmented } from "../components/Segmented";
import { Toggle } from "../components/Toggle";
import { useLabsStore } from "../../../lib/labs";
import { PALETTES, THEMES } from "../types";

const TIME_FORMATS: { value: TimeFormat; label: string }[] = [
  { value: "24h", label: "24-hour" },
  { value: "12h", label: "12-hour" },
];

// General owns appearance + the absorbed About content. Defaults moved to
// their sections (#15): language → Transcription, preset → Summaries.
export function GeneralTab() {
  const theme = useThemeStore((t) => t.theme);
  const setThemePref = useThemeStore((t) => t.setTheme);
  const palette = usePaletteStore((p) => p.palette);
  const setPalettePref = usePaletteStore((p) => p.setPalette);
  const timeFormat = useTimeFormatStore((t) => t.format);
  const setTimeFormat = useTimeFormatStore((t) => t.setFormat);
  const feedsOn = useLabsStore((s) => s.flags.labs_feeds);
  const setLab = useLabsStore((s) => s.set);

  // The description is the selected design's own, so the row explains what is
  // actually on screen instead of describing the picker.
  const design = PALETTE_REGISTRY.find((p) => p.id === palette);

  return (
    <>
      <Section title="Appearance">
        <Row
          label="Design"
          description={
            design
              ? `${design.description} Each design has its own light and dark mode.`
              : "Typeface, type scale and colours. Each design has its own light and dark mode."
          }
          control={
            <Segmented
              label="Design"
              value={palette}
              onChange={setPalettePref}
              options={PALETTES}
            />
          }
        />
        <Row
          label="Theme"
          description="System follows macOS appearance."
          control={
            <Segmented
              label="Theme"
              value={theme}
              onChange={setThemePref}
              options={THEMES}
            />
          }
        />
        <Row
          label="Time format"
          description={
            timeFormat === "12h"
              ? "Clock times show as 12-hour with AM/PM (2:30 PM)."
              : "Clock times show as 24-hour (14:30)."
          }
          control={
            <Segmented
              label="Time format"
              value={timeFormat}
              onChange={(v) => void setTimeFormat(v)}
              options={TIME_FORMATS}
            />
          }
        />
      </Section>

      {/* Labs: features that ship hidden. Off by default; each one adds its
          own entry to the sidebar when switched on. */}
      <Section title="Labs">
        <Row
          label="Feeds"
          description="An RSS and podcast reader in the sidebar. Podcast episodes can be downloaded and transcribed into notes in a Podcasts folder. Early — expect rough edges."
          control={
            <Toggle checked={feedsOn} onChange={(next) => void setLab("labs_feeds", next)} label="Enable Feeds" />
          }
        />
      </Section>
    </>
  );
}
