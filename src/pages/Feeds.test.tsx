import { describe, it, expect, beforeEach, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { Feeds, downloadLabel, formatBytes, formatDuration } from "./Feeds";
import { mockTauri } from "../test/tauri";
import { demoFeedItems, demoFeeds } from "../test/feeds";

const openExternal = vi.fn();
vi.mock("@tauri-apps/plugin-shell", () => ({ open: (u: string) => openExternal(u) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

function setup(over: Record<string, (a: unknown) => unknown> = {}, route = "/feeds") {
  const calls: { cmd: string; args: unknown }[] = [];
  const track =
    (cmd: string, fn: (a: unknown) => unknown) =>
    (a: unknown) => {
      calls.push({ cmd, args: a });
      return fn(a);
    };
  mockTauri({
    feeds_list: track("feeds_list", () => demoFeeds),
    feed_items_list: track("feed_items_list", (a) => {
      const { feedId, unreadOnly } = (a ?? {}) as { feedId?: string | null; unreadOnly?: boolean };
      return demoFeedItems.filter((i) => (!feedId || i.feed_id === feedId) && (!unreadOnly || !i.read));
    }),
    feed_item_set_read: track("feed_item_set_read", () => null),
    feeds_refresh: track("feeds_refresh", () => 3),
    feeds_add: track("feeds_add", () => demoFeeds[0]),
    ...Object.fromEntries(Object.entries(over).map(([k, v]) => [k, track(k, v)])),
  });
  render(
    <MemoryRouter initialEntries={[route]}>
      <Routes>
        <Route path="/feeds" element={<Feeds />} />
        <Route path="/feeds/:feedId" element={<Feeds />} />
        <Route path="/note/:id" element={<p>note page</p>} />
      </Routes>
    </MemoryRouter>,
  );
  return calls;
}

describe("Feeds page", () => {
  beforeEach(() => openExternal.mockReset());

  it("lists feeds with unread counts and a failing feed's error", async () => {
    setup();
    const nav = await screen.findByRole("navigation", { name: "Feeds" });
    expect(within(nav).getByText("Databricks Blog")).toBeInTheDocument();
    expect(within(nav).getByRole("button", { name: "Old Newsletter" })).toHaveAttribute(
      "title",
      expect.stringMatching(/Last refresh failed: .*404/),
    );
    expect(screen.getByText("3 unread")).toBeInTheDocument();
  });

  it("opening an item marks it read and renders the sanitized article", async () => {
    const calls = setup();
    const items = await screen.findByRole("region", { name: "Items" });
    expect(within(items).getAllByLabelText("Unread")).toHaveLength(3);
    fireEvent.click(within(items).getByText(/Data Modeling Best Practices/));
    const reader = screen.getByRole("article", { name: "Reader" });
    expect(within(reader).getByRole("heading", { name: /Data Modeling/ })).toBeInTheDocument();
    expect(within(reader).getByText("Start from identity")).toBeInTheDocument();
    expect(within(items).getAllByLabelText("Unread")).toHaveLength(2);
    expect(calls).toContainEqual({ cmd: "feed_item_set_read", args: { id: "i1", read: true } });
  });

  it("article links open in the system browser, not the webview", async () => {
    setup();
    fireEvent.click(await screen.findByText(/Data Modeling Best Practices/));
    fireEvent.click(screen.getByRole("link", { name: "The docs" }));
    expect(openExternal).toHaveBeenCalledWith("https://docs.databricks.com/");
  });

  it("j / k walk items and m toggles read", async () => {
    const calls = setup();
    await screen.findByText(/Data Modeling Best Practices/);
    fireEvent.keyDown(window, { key: "j" });
    expect(screen.getByRole("article", { name: "Reader" })).toHaveTextContent(/Data Modeling/);
    fireEvent.keyDown(window, { key: "j" });
    expect(screen.getByRole("article", { name: "Reader" })).toHaveTextContent(/Episode 42/);
    expect(screen.getByRole("article", { name: "Reader" })).toHaveTextContent("Episode · 57 min");
    fireEvent.keyDown(window, { key: "m" });
    expect(calls).toContainEqual({ cmd: "feed_item_set_read", args: { id: "i2", read: false } });
  });

  it("scopes items to the routed feed", async () => {
    const calls = setup({}, "/feeds/f-pod");
    await screen.findByText(/Episode 42/);
    expect(screen.queryByText(/Data Modeling/)).toBeNull();
    expect(calls.find((c) => c.cmd === "feed_items_list")?.args).toMatchObject({ feedId: "f-pod" });
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Data Brew");
  });

  it("refresh reports how many items are new", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Refresh" }));
    expect(await screen.findByRole("status")).toHaveTextContent("3 new items.");
  });

  it("shows the empty state with no feeds, and adding one subscribes", async () => {
    const calls = setup({ feeds_list: () => [] });
    expect(await screen.findByText("Follow blogs and podcasts")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: /Add feed/ })[0]);
    const dialog = screen.getByRole("dialog", { name: "Add a feed" });
    fireEvent.change(within(dialog).getByLabelText("Feed or website address"), {
      target: { value: "databricks.com" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Subscribe" }));
    await waitFor(() => expect(calls).toContainEqual({ cmd: "feeds_add", args: { url: "databricks.com" } }));
  });

  it("surfaces an add failure inline", async () => {
    setup({
      feeds_add: () => {
        throw "That page doesn't advertise a feed.";
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: /Add feed/ }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Feed or website address"), { target: { value: "x.com" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Subscribe" }));
    expect(await within(dialog).findByText(/doesn't advertise a feed/)).toBeInTheDocument();
  });
});

describe("Transcribing an episode", () => {
  it("offers Transcribe on an episode, and files it as a note", async () => {
    const calls = setup({
      settings_get: () => "en",
      feed_item_transcribe: () => ({
        note: { id: "n-42", title: "Episode 42", folder_id: "fo-pod" },
        folder: { id: "fo-pod", name: "Podcasts", created_at: 0, updated_at: 0, workspace_id: "" },
      }),
    });
    fireEvent.click(await screen.findByText(/Episode 42/));
    const reader = screen.getByRole("article", { name: "Reader" });
    expect(reader).toHaveTextContent("57 min · 46 MB");
    fireEvent.click(within(reader).getByRole("button", { name: /Transcribe episode/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(/Episode 42: Lakebase in production/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Download & transcribe" }));
    expect(await screen.findByText("note page")).toBeInTheDocument();
    expect(calls).toContainEqual({
      cmd: "feed_item_transcribe",
      args: { id: "i2", language: "en", expectedSpeakers: null },
    });
  });

  it("an already-transcribed episode links to its note instead", async () => {
    setup();
    fireEvent.click(await screen.findByText(/Episode 41/));
    const reader = screen.getByRole("article", { name: "Reader" });
    expect(within(reader).queryByRole("button", { name: /Transcribe episode/ })).toBeNull();
    fireEvent.click(within(reader).getByRole("button", { name: /Open transcript/ }));
    expect(await screen.findByText("note page")).toBeInTheDocument();
  });

  it("keeps the dialog open with the error when the recorder is busy", async () => {
    setup({
      feed_item_transcribe: () => {
        throw "already recording or importing";
      },
    });
    fireEvent.click(await screen.findByText(/Episode 42/));
    fireEvent.click(screen.getByRole("button", { name: /Transcribe episode/ }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Download & transcribe" }));
    expect(await within(dialog).findByText("already recording or importing")).toBeInTheDocument();
  });

  it("articles without audio offer no transcribe", async () => {
    setup();
    fireEvent.click(await screen.findByText(/Data Modeling/));
    expect(screen.queryByRole("button", { name: /Transcribe episode/ })).toBeNull();
  });
});

describe("download progress label", () => {
  it("reads naturally at each stage", () => {
    expect(downloadLabel(null)).toBe("Starting download…");
    expect(downloadLabel({ received: 25, total: 100 })).toBe("Downloading… 25%");
    expect(downloadLabel({ received: 5 * 1024 * 1024, total: 0 })).toBe("Downloading… 5 MB");
    expect(downloadLabel({ received: 100, total: 100 })).toBe("Starting transcription…");
    expect(formatBytes(48_000_000)).toBe("46 MB");
    expect(formatBytes(3 * 1024 ** 3)).toBe("3.0 GB");
  });
});

describe("formatDuration", () => {
  it("formats episode lengths", () => {
    expect(formatDuration(0)).toBe("");
    expect(formatDuration(20)).toBe("1 min");
    expect(formatDuration(3420)).toBe("57 min");
    expect(formatDuration(3723)).toBe("1 h 2 min");
  });
});
