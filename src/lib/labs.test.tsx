import { describe, it, expect, beforeEach } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { Sidebar } from "../components/Sidebar";
import { mockTauri } from "../test/tauri";
import { useLabsStore } from "./labs";

// Labs flags gate hidden features. Feeds is the first: its sidebar entry must
// not exist until the flag is on, and must come and go with the toggle.
function renderSidebar() {
  return render(
    <MemoryRouter initialEntries={["/"]}>
      <Sidebar onCollapse={() => {}} />
    </MemoryRouter>,
  );
}

describe("Labs: Feeds", () => {
  beforeEach(() => {
    localStorage.clear();
    mockTauri();
    useLabsStore.setState({ flags: { labs_feeds: false } });
  });

  it("hides the Feeds nav item by default", () => {
    renderSidebar();
    expect(screen.queryByRole("link", { name: /feeds/i })).toBeNull();
  });

  it("shows it once the flag is switched on, and persists the choice", async () => {
    const writes: unknown[] = [];
    mockTauri({ settings_set: (a) => (writes.push(a), null) });
    renderSidebar();
    await act(() => useLabsStore.getState().set("labs_feeds", true));
    expect(screen.getByRole("link", { name: /feeds/i })).toHaveAttribute("href", "/feeds");
    expect(localStorage.getItem("chatham.labs_feeds")).toBe("true");
    expect(writes).toContainEqual({ key: "labs_feeds", value: "true" });
  });

  it("hydrates from the stored setting", async () => {
    mockTauri({ settings_get: (a) => ((a as { key: string }).key === "labs_feeds" ? "true" : null) });
    await useLabsStore.getState().hydrate();
    expect(useLabsStore.getState().flags.labs_feeds).toBe(true);
  });
});
