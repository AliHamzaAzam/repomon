# Agents

repomon runs every agent the same way — in a durable tmux window per lane — but it learns
each agent's *status* differently, because each CLI stores its session state differently.

## How agents run

When you spawn an agent (New Lane, the `e` key, or `agent.spawn`), the daemon launches its
CLI in a tmux window named `lane-<id>` inside the configured session (default `repomon`):

```
tmux new-window -t repomon -n lane-7 -c <worktree> '<agent-binary> [task]'
```

The daemon reads output with `capture-pane`, sends input with `send-keys`, and `attach`
gives you the raw session. Because tmux owns the process, the agent survives the daemon and
the TUI. The spawned kind is recorded on the lane so repomon can identify it later.

`AgentKind::command()` maps kinds to binaries:

| Kind          | Binary         |
|---------------|----------------|
| `claude-code` | `claude`       |
| `codex`       | `codex`        |
| `aider`       | `aider`        |
| `cursor`      | `cursor-agent` |
| other         | the kind string itself |

## How status is detected

Each agent kind has an `AgentMonitor` (`crates/repomon-core/src/agent/`). Monitors are tried
in priority order; the first to return a summary wins. If none does, the daemon falls back to
"is the repomon-spawned tmux window alive?" and shows the recorded kind as **Running**.

### Claude Code — rich status

Transcripts live at `~/.claude/projects/<encoded-cwd>/<session>.jsonl`, where the directory
name is the working directory with `/` and `.` replaced by `-`. repomon derives:

- **tool-call count** — `tool_use` blocks across assistant messages,
- **status** — *Waiting* (the last entry is an assistant turn with no tool call → **needs you**),
  *Running* (mid tool-loop), or *Idle* (no activity for 10 min),
- **title** — first user message or a `summary` entry.

The encoding scheme has changed before, so it's isolated in `claude::encode_project_dir` and
fixture-tested; matching also falls back to the `cwd` recorded inside each transcript.

### Aider — coarse status

Aider writes `.aider.chat.history.md` into the working directory. repomon uses that file's
modification time: **Running** if it changed in the last two minutes, else **Idle**. (There's
no reliable "needs you" signal yet.)

### Codex — tmux-only for now

Codex's on-disk session format isn't stable enough to parse reliably, so `CodexMonitor`
returns nothing and repomon relies on the tmux-alive fallback for Codex agents it spawned.
When the format stabilizes, implement `CodexMonitor::summary_for` like the others.

## Adding a new agent

1. Add a variant (or use `Other`) and a binary in `AgentKind` (`model.rs`).
2. Implement `AgentMonitor` for it in `crates/repomon-core/src/agent/` and add it to
   `default_monitors()`.
3. Add it to `AGENT_KINDS` in the TUI so New Lane can spawn it (Tab to cycle).
