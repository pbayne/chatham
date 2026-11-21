import { useEffect } from "react";
import { create } from "zustand";
import { ipc } from "./ipc";

// Clock display: 24-hour ("14:30") or 12-hour with AM/PM ("2:30 PM").
// A UI-only preference — stored in SQLite (source of truth) with a localStorage
// mirror so the synchronous formatters (formatNoteDate, session labels) can read
// it during render without awaiting an IPC round-trip.
export type TimeFormat = "24h" | "12h";

export const DEFAULT_TIME_FORMAT: TimeFormat = "24h";

export function isTimeFormat(v: unknown): v is TimeFormat {
  return v === "24h" || v === "12h";
}

const CACHE_KEY = "chatham.time_format";

// Synchronous read of the current preference, backed by the localStorage mirror.
// Returns true when times should render 12-hour (hour12). Used by the plain
// formatting helpers, which run in render and cannot await the store.
export function useHour12(): boolean {
  return useTimeFormatStore((s) => s.format === "12h");
}

// Non-hook synchronous accessor for the formatters in noteList.ts / sessions.ts,
// which are plain functions (not components). Reads the localStorage mirror so
// it stays correct across the whole app the instant setFormat writes it.
export function hour12Now(): boolean {
  try {
    return localStorage.getItem(CACHE_KEY) === "12h";
  } catch {
    return DEFAULT_TIME_FORMAT === "12h";
  }
}

function cache(format: TimeFormat) {
  try {
    localStorage.setItem(CACHE_KEY, format);
  } catch {
    /* private mode / quota — the mirror is an optimisation, never load-bearing */
  }
}

type TimeFormatState = {
  format: TimeFormat;
  setFormat: (f: TimeFormat) => Promise<void>;
  hydrate: () => Promise<void>;
};

export const useTimeFormatStore = create<TimeFormatState>((set) => ({
  format: (() => {
    try {
      const c = localStorage.getItem(CACHE_KEY);
      return isTimeFormat(c) ? c : DEFAULT_TIME_FORMAT;
    } catch {
      return DEFAULT_TIME_FORMAT;
    }
  })(),
  setFormat: async (format) => {
    cache(format);
    set({ format });
    await ipc.setSetting("time_format", format);
  },
  hydrate: async () => {
    const stored = await ipc.getSetting("time_format");
    const format = isTimeFormat(stored) ? stored : DEFAULT_TIME_FORMAT;
    cache(format);
    set({ format });
  },
}));

export function useTimeFormatBoot() {
  const hydrate = useTimeFormatStore((s) => s.hydrate);
  useEffect(() => {
    hydrate();
  }, [hydrate]);
}
