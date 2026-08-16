import { describe, expect, it } from "vitest";
import { translateError } from "./errors";

describe("translateError", () => {
  it("translates macOS ENOENT spawn failures with tmux context", () => {
    const raw = "failed to spawn child: No such file or directory (os error 2)";
    const res = translateError(raw, { binary: "tmux" });
    expect(res.friendly).toBe(
      "tmux isn't installed or couldn't be found — Repomon needs tmux to run agent sessions",
    );
    expect(res.raw).toBe(raw);
    expect(res.isMissingBinary).toBe(true);
  });

  it("translates a tmux-missing agent.spawn error from raw message", () => {
    const raw = "DaemonRpcError: tmux: No such file or directory (os error 2)";
    const res = translateError(raw);
    expect(res.friendly).toBe(
      "tmux isn't installed or couldn't be found — Repomon needs tmux to run agent sessions",
    );
    expect(res.raw).toBe(raw);
    expect(res.isMissingBinary).toBe(true);
  });

  it("translates a git-missing lane.create error", () => {
    const raw = "failed to run git worktree add: No such file or directory (os error 2)";
    const res = translateError(raw, { binary: "git" });
    expect(res.friendly).toBe(
      "git isn't installed or couldn't be found — Repomon needs git to manage repositories and worktrees",
    );
    expect(res.raw).toBe(raw);
    expect(res.isMissingBinary).toBe(true);
  });

  it("translates git missing from raw string without explicit context", () => {
    const raw = "git: command not found";
    const res = translateError(raw);
    expect(res.friendly).toBe(
      "git isn't installed or couldn't be found — Repomon needs git to manage repositories and worktrees",
    );
    expect(res.isMissingBinary).toBe(true);
  });

  it("translates missing custom agent command", () => {
    const raw = "failed to spawn custom-agent: No such file or directory (os error 2)";
    const res = translateError(raw, { command: "custom-agent" });
    expect(res.friendly).toBe("'custom-agent' isn't installed or not on PATH");
    expect(res.raw).toBe(raw);
    expect(res.isMissingBinary).toBe(true);
  });

  it("translates missing cursor-agent command from context or message", () => {
    const raw = "cursor-agent: No such file or directory (os error 2)";
    const res = translateError(raw, { agent: "cursor" });
    expect(res.friendly).toBe("'cursor' isn't installed or not on PATH");
    expect(res.isMissingBinary).toBe(true);
  });

  it("translates general ENOENT error when no specific binary is matched", () => {
    const raw = "No such file or directory (os error 2)";
    const res = translateError(raw);
    expect(res.friendly).toBe(
      "A required command or file could not be found (No such file or directory)",
    );
    expect(res.raw).toBe(raw);
    expect(res.isMissingBinary).toBe(true);
  });

  it("passes through non-ENOENT errors unmodified", () => {
    const raw = "fatal: a branch named 'feature/test' already exists";
    const res = translateError(raw);
    expect(res.friendly).toBe(raw);
    expect(res.raw).toBe(raw);
    expect(res.isMissingBinary).toBe(false);
  });

  it("handles Error instances and empty/null inputs", () => {
    const err = new Error("Custom RPC failure: conflict");
    const res = translateError(err);
    expect(res.friendly).toBe("Custom RPC failure: conflict");
    expect(res.isMissingBinary).toBe(false);

    const empty = translateError(null);
    expect(empty.friendly).toBe("An unexpected error occurred");
  });
});
