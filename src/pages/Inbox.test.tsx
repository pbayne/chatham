import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { mockTauri } from "../test/tauri";
import { Inbox } from "./Inbox";

describe("Inbox", () => {
  it("renders the empty board when the backend answers null", async () => {
    mockTauri({
      inbox_status: () => ({ connected: true, provider: "local", auth_mode: "oauth", triaged_count: 0 }),
      inbox_list: () => null,
    });
    render(
      <MemoryRouter>
        <Inbox />
      </MemoryRouter>,
    );
    expect(await screen.findByText("No triaged mail yet.")).toBeInTheDocument();
  });
});
