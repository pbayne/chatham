import { useEffect } from "react";
import { create } from "zustand";
import { ipc } from "./ipc";

// Labs: features that ship hidden until switched on in Settings → General.
// Each flag is an ordinary setting ("true" enables; anything else is off) with
// a localStorage mirror so the sidebar can render the right nav on first paint
// instead of flashing an item in or out after the IPC round-trip.
export type LabsFlag = "labs_feeds";
export const LABS_FLAGS: LabsFlag[] = ["labs_feeds"];

const cacheKey = (f: LabsFlag) => `chatham.${f}`;

function readCache(f: LabsFlag): boolean {
  try {
    return localStorage.getItem(cacheKey(f)) === "true";
  } catch {
    return false;
  }
}

function writeCache(f: LabsFlag, on: boolean) {
  try {
    localStorage.setItem(cacheKey(f), on ? "true" : "false");
  } catch {
    /* the mirror is an optimisation, never load-bearing */
  }
}

type LabsState = {
  flags: Record<LabsFlag, boolean>;
  set: (f: LabsFlag, on: boolean) => Promise<void>;
  hydrate: () => Promise<void>;
};

export const useLabsStore = create<LabsState>((set) => ({
  flags: Object.fromEntries(LABS_FLAGS.map((f) => [f, readCache(f)])) as Record<LabsFlag, boolean>,
  set: async (f, on) => {
    writeCache(f, on);
    set((s) => ({ flags: { ...s.flags, [f]: on } }));
    await ipc.setSetting(f, on ? "true" : "false");
  },
  hydrate: async () => {
    const entries = await Promise.all(
      LABS_FLAGS.map(async (f) => [f, (await ipc.getSetting(f)) === "true"] as const),
    );
    for (const [f, on] of entries) writeCache(f, on);
    set({ flags: Object.fromEntries(entries) as Record<LabsFlag, boolean> });
  },
}));

export function useLabs(f: LabsFlag): boolean {
  return useLabsStore((s) => s.flags[f]);
}

export function useLabsBoot() {
  const hydrate = useLabsStore((s) => s.hydrate);
  useEffect(() => {
    void hydrate();
  }, [hydrate]);
}
