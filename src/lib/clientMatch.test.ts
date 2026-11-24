import { describe, it, expect, vi } from "vitest";
import { findClientByName, resolveClientId } from "./clientMatch";

describe("findClientByName", () => {
  const clients = [
    { id: "1", name: "Acme" },
    { id: "2", name: "Globex Corp" },
  ];

  it("matches case-insensitively and trims", () => {
    expect(findClientByName(clients, "acme")?.id).toBe("1");
    expect(findClientByName(clients, "  ACME  ")?.id).toBe("1");
    expect(findClientByName(clients, "Globex Corp")?.id).toBe("2");
  });

  it("returns undefined for no match or empty input", () => {
    expect(findClientByName(clients, "Initech")).toBeUndefined();
    expect(findClientByName(clients, "")).toBeUndefined();
    // Does NOT fuzzy-match a near-name (distinct clients stay distinct).
    expect(findClientByName(clients, "Acme Inc")).toBeUndefined();
  });
});

describe("resolveClientId", () => {
  it("reuses an existing client and never creates", async () => {
    const list = vi.fn().mockResolvedValue([{ id: "1", name: "Acme" }]);
    const create = vi.fn();
    const id = await resolveClientId("acme", list, create);
    expect(id).toBe("1");
    expect(create).not.toHaveBeenCalled();
  });

  it("creates a new client when none matches", async () => {
    const list = vi.fn().mockResolvedValue([{ id: "1", name: "Acme" }]);
    const create = vi.fn().mockResolvedValue({ id: "9", name: "Initech" });
    const id = await resolveClientId("Initech", list, create);
    expect(id).toBe("9");
    expect(create).toHaveBeenCalledWith("Initech");
  });
});
