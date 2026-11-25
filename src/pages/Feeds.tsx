import { Rss } from "lucide-react";

// Feeds (Labs, docs/design/feeds-labs.md): an RSS reader whose podcast
// episodes download and transcribe into ordinary notes in a Podcasts folder.
// Hidden behind Settings → General → Labs. This first cut is the page frame;
// the feed list, reader and episode transcription land in the following phases.
export function Feeds() {
  return (
    <div className="h-full overflow-y-auto nd-well">
      <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-24">
        <div className="flex items-center gap-2.5 mb-1 px-1">
          <Rss size={22} strokeWidth={1.6} className="text-[var(--color-interactive)]" />
          <h1 className="nd-heading truncate">Feeds</h1>
          <span className="nd-chip ml-1">Labs</span>
        </div>
        <p className="text-sm text-[var(--color-text-muted)] mb-6 px-1">
          Follow blogs and podcasts. A podcast episode can be downloaded and transcribed into a note,
          where search, chat and summaries work on it like any meeting.
        </p>
        <div className="flex flex-col items-center justify-center py-20 text-center">
          <Rss size={40} strokeWidth={1.3} className="text-[var(--color-text-disabled)] mb-3" />
          <p className="text-sm text-[var(--color-text-muted)]">No feeds yet.</p>
        </div>
      </div>
    </div>
  );
}
