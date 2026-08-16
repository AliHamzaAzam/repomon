export interface ErrorContext {
  binary?: string;
  command?: string;
  agent?: string;
}

export interface TranslatedError {
  /** User-friendly message explaining what went wrong */
  friendly: string;
  /** The raw error string for technical details / debugging */
  raw?: string;
  /** True when the error was classified as a missing binary / executable */
  isMissingBinary: boolean;
}

function extractRawString(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof (error as { message: unknown }).message === "string"
  ) {
    return (error as { message: string }).message;
  }
  return String(error ?? "");
}

/**
 * Translates low-level daemon/tauri errors (such as OS ENOENT or missing binaries)
 * into clear, actionable human copy while preserving the raw error for technical details.
 */
export function translateError(
  error: unknown,
  contextInput?: ErrorContext | string,
): TranslatedError {
  const raw = extractRawString(error).trim();
  const context: ErrorContext =
    typeof contextInput === "string"
      ? { command: contextInput, binary: contextInput }
      : (contextInput ?? {});

  const lowerRaw = raw.toLowerCase();

  const isEnoent =
    lowerRaw.includes("no such file or directory") ||
    lowerRaw.includes("os error 2") ||
    lowerRaw.includes("enoent") ||
    lowerRaw.includes("not found") ||
    lowerRaw.includes("cannot find the path specified") ||
    lowerRaw.includes("executable file not found");

  if (!isEnoent) {
    return {
      friendly: raw || "An unexpected error occurred",
      raw: raw || undefined,
      isMissingBinary: false,
    };
  }

  const notFoundMatch = raw.match(/^(?:.*:\s*)?([a-zA-Z0-9_\-\.]+):\s*(?:command\s+)?not found/i);
  if (notFoundMatch && notFoundMatch[1]) {
    const cmd = notFoundMatch[1];
    if (cmd === "tmux") {
      return {
        friendly: "tmux isn't installed or couldn't be found — Repomon needs tmux to run agent sessions",
        raw,
        isMissingBinary: true,
      };
    }
    if (cmd === "git") {
      return {
        friendly: "git isn't installed or couldn't be found — Repomon needs git to manage repositories and worktrees",
        raw,
        isMissingBinary: true,
      };
    }
    return {
      friendly: `'${cmd}' isn't installed or not on PATH`,
      raw,
      isMissingBinary: true,
    };
  }

  const binary = context.binary || context.command || context.agent;

  if (binary === "tmux" || lowerRaw.includes("tmux")) {
    return {
      friendly: "tmux isn't installed or couldn't be found — Repomon needs tmux to run agent sessions",
      raw,
      isMissingBinary: true,
    };
  }

  if (binary === "git" || lowerRaw.includes("git")) {
    return {
      friendly: "git isn't installed or couldn't be found — Repomon needs git to manage repositories and worktrees",
      raw,
      isMissingBinary: true,
    };
  }

  if (binary) {
    return {
      friendly: `'${binary}' isn't installed or not on PATH`,
      raw,
      isMissingBinary: true,
    };
  }

  const knownBinaries = ["claude", "cursor-agent", "codex", "opencode", "antigravity", "agy", "aider"];
  for (const known of knownBinaries) {
    if (lowerRaw.includes(known)) {
      return {
        friendly: `'${known}' isn't installed or not on PATH`,
        raw,
        isMissingBinary: true,
      };
    }
  }

  return {
    friendly: "A required command or file could not be found (No such file or directory)",
    raw,
    isMissingBinary: true,
  };
}
