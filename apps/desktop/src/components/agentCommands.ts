import type { CatalogCommand, CommandCatalog } from "../ipc/rpc";

// Match the daemon's command grammar. Paths and multiline prompts stay ordinary messages.
export function isAgentCommand(text: string): boolean {
  return /^\/[a-z][a-z0-9_-]*(?:[ \t]+[^\r\n]*)?$/i.test(text.trim());
}

// "plugin-name:command" -> "command", the bare alias the reference screenshot shows in
// parentheses beside the namespaced form. A name without a namespace is its own bare alias.
export function bareAlias(name: string): string {
  const idx = name.lastIndexOf(":");
  return idx === -1 ? name : name.slice(idx + 1);
}

export interface CommandResolution {
  // A fully specified line to send as ordinary input (agent.send_input) - no terminal, no
  // interactive state to steer.
  oneShotLine?: string;
  // The full text to open the terminal route with instead, when a one-shot form isn't known to
  // be safe. Carries whatever the operator actually typed, arguments included.
  terminalText?: string;
}

function findCommand(commands: CatalogCommand[], name: string): CatalogCommand | undefined {
  const lower = name.toLowerCase();
  return commands.find((entry) => entry.name.toLowerCase() === lower || bareAlias(entry.name).toLowerCase() === lower);
}

// Never drive the agent's interactive picker blind: a command the catalog marks one_shot is sent
// as a single fully specified line. A command the catalog does not recognize at all is not one we
// are sure is safe to fire blind either, so it takes the same terminal route as one_shot:false -
// the conservative default, not a guess.
export function resolveCommand(text: string, catalog: CommandCatalog): CommandResolution {
  const trimmed = text.trim();
  const match = /^\/([a-z][a-z0-9_-]*)(?:[ \t]+[^\r\n]*)?$/i.exec(trimmed);
  if (!match) return { terminalText: trimmed };
  const entry = findCommand(catalog.commands, match[1]);
  return entry?.one_shot ? { oneShotLine: trimmed } : { terminalText: trimmed };
}
