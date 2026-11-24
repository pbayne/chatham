// Resolve a harvested company name ("Acme", "Globex Corp") to a client id,
// reusing an existing client when one matches case-insensitively and creating
// one otherwise. Kept separate from the UI so the match rule is unit-testable.

export type ClientLike = { id: string; name: string };

/**
 * Find an existing client whose name matches `company` (case-insensitive, after
 * trimming). Returns the client, or undefined when none matches. Exact-ish by
 * design: fuzzy company matching ("Acme" vs "Acme Inc.") would wrongly merge
 * distinct clients, so the user confirms a near-miss by picking in the picker.
 */
export function findClientByName(
  clients: ClientLike[],
  company: string,
): ClientLike | undefined {
  const needle = company.trim().toLowerCase();
  if (!needle) return undefined;
  return clients.find((c) => c.name.trim().toLowerCase() === needle);
}

/**
 * Resolve a company to a client id: reuse a case-insensitive name match, else
 * create a new client. `list`/`create` are injected (the ipc calls) so this is
 * testable without a backend. Returns the client id to set on the note.
 */
export async function resolveClientId(
  company: string,
  list: () => Promise<ClientLike[]>,
  create: (name: string) => Promise<ClientLike>,
): Promise<string> {
  const name = company.trim();
  const existing = findClientByName(await list(), name);
  if (existing) return existing.id;
  const made = await create(name);
  return made.id;
}
