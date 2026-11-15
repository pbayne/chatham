import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup, configure } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";

// findBy*'s default 1s can flake under parallel test workers when a
// component chains async loads (cloud status → members). Generous ceiling;
// passing tests don't get slower, only genuinely-absent elements do.
configure({ asyncUtilTimeout: 4000 });

// The event plugin unregisters listeners through this internal, which
// mockIPC doesn't provide.
(window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};

afterEach(() => {
  cleanup();
  // Don't clearMocks(): unmount cleanups (event unlisten etc.) resolve
  // async after afterEach and need a live invoke. Park a benign handler
  // instead; each test installs its own via mockTauri().
  mockIPC(async () => null);
});

// Radix primitives (#114) lean on browser APIs jsdom doesn't implement:
// Floating UI measures the anchor with a ResizeObserver, menus/selects call
// scrollIntoView when roving focus, and Select's trigger uses pointer capture
// to distinguish a click from a drag. Absent, every popover test throws before
// it can assert anything.
if (!("ResizeObserver" in window)) {
  (window as unknown as Record<string, unknown>).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
}

// jsdom lacks matchMedia; theme/palette boot and CSS hooks touch it.
if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}

// Node 26 ships an experimental native `localStorage` that THROWS on access
// unless the process was started with `--localstorage-file`, and it shadows
// jsdom's. Tests that call `localStorage.clear()` in beforeEach (visibility,
// export-modal, workspace-move, …) crash before the first assertion. Install a
// deterministic in-memory implementation unconditionally so tests never depend
// on either the Node flag or jsdom's storage. Defined on both globalThis and
// window since app code reaches it through both.
{
  const store = new Map<string, string>();
  const mem: Storage = {
    get length() {
      return store.size;
    },
    clear() {
      store.clear();
    },
    getItem(key: string) {
      return store.has(key) ? (store.get(key) as string) : null;
    },
    key(index: number) {
      return Array.from(store.keys())[index] ?? null;
    },
    removeItem(key: string) {
      store.delete(key);
    },
    setItem(key: string, value: string) {
      store.set(key, String(value));
    },
  };
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: mem,
  });
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: mem,
  });
}
