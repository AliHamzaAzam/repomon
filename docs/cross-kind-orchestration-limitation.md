# Cross-kind agent orchestration: architecture & limitations

Updated 2026-08-16 (Track A: cross-kind orchestration, A5: universal fleet-mail invariant).

## Goal

Any agent kind managing any other agent kind natively via MCP tool calls:
claude -> antigravity, codex -> claude, antigravity -> codex, opencode -> claude, etc. - any combination.

## Current status by agent kind

| Agent Kind | Worker Fleet Mail | Orchestrator Backend | Transcript Parsing |
|------------|-------------------|----------------------|--------------------|
| **Claude Code** (`claude`) | ✓ Supported (`--mcp-config`) | ✓ Supported (`build_claude_orchestrator_command`) | ✓ Full on-disk JSONL transcript (`~/.claude/projects/`) |
| **Codex** (`codex`) | ✓ Supported (`-c mcp_servers.repomon...`) | ✓ Supported (`build_codex_orchestrator_command`) | Degraded (empty chat; pane view) |
| **Antigravity** (`agy`) | ✓ Supported (`~/.gemini/config/mcp_config.json`) | ✓ Supported (`build_antigravity_orchestrator_command`) | Degraded (empty chat; pane view) |
| **OpenCode** (`opencode`) | ✓ Supported (`OPENCODE_CONFIG_CONTENT`) | ✓ Supported (`build_opencode_orchestrator_command`) | Degraded (empty chat; pane view) |
| **Cursor** (`cursor-agent`) | ✓ Supported (`~/.cursor/mcp.json`) | ✗ Not supported | Degraded (pane view) |
| **Aider** (`aider`) | ✗ Not wireable (no native MCP support) | ✗ Not supported | Degraded (mtime only) |
| **Custom/Other** | ✓ Dialect-routed (see below) | ✗ Not supported | None |

## Coverage matrix: spawn × adopt × wiring pillars (A5 audit)

All wired kinds must satisfy four pillars per path:
**(a)** MCP registration mechanism, **(b)** `REPOMON_MCP_MODE=agent`, **(c)** `REPOMON_MCP_SOCKET`, **(d)** identity token in env.

| Kind | spawn (a) mech | spawn (b)(c)(d) env | adopt (a) mech | adopt (b)(c)(d) env | adopt re-launchable? |
|------|----------------|---------------------|----------------|---------------------|----------------------|
| ClaudeCode | `attach_agent_mcp` (`--mcp-config`) | ✓ uniform | `attach_agent_mcp` | ✓ uniform | ✓ (`--resume <sid>` / `--continue`) |
| Codex | `attach_agent_mcp` (`-c mcp_servers`) | ✓ uniform | `attach_agent_mcp` | ✓ uniform | ✓ (fresh launch, no session-resume flag) |
| OpenCode | `configure_backend_mcp` (`OPENCODE_CONFIG_CONTENT`) | ✓ uniform | `configure_backend_mcp` | ✓ uniform | ✓ (`--session <sid>` / `--continue`) |
| Antigravity | `configure_backend_mcp` (`~/.gemini/mcp_config.json`) | ✓ uniform | `configure_backend_mcp` | ✓ uniform | ✓ (`--conversation <sid>` / `--continue`) |
| Cursor | `configure_backend_mcp` (`~/.cursor/mcp.json`) | ✓ uniform | `configure_backend_mcp` | ✓ uniform | ✓ (fresh launch, no session-resume flag) |
| Aider | ✗ no MCP support | ✓ uniform (env set, no registration) | ✗ no MCP support | ✓ uniform | ✓ (fresh launch) |
| Other/custom | Dialect-routed (see below) | ✓ uniform | Dialect-routed | ✓ uniform | ✓ (fresh launch) |

**Env uniformity note**: pillars (b)(c)(d) — `REPOMON_MCP_MODE=agent`, `REPOMON_MCP_SOCKET`, and `REPOMON_MCP_IDENTITY_TOKEN` — are set unconditionally for ALL kinds on both paths before `configure_backend_mcp` is called. This is by design: even Aider and unknown custom agents carry the correct socket/mode/token in their process environment in case a future version gains MCP support or the operator wraps the binary.

## A5 fixes applied (2026-08-16)

**Before A5**, the `agent.adopt` path's kind-dispatch `match` fell through to `program: String::new()` (returning "cannot be adopted") for: Codex, Cursor, Aider, and Other/custom. Only ClaudeCode, OpenCode, and Antigravity could be adopted.

**After A5**:
- All seven kind categories are now adoptable. Codex, Cursor, and Aider re-launch fresh in the worktree (no session-resume flag exists for them). Other/custom re-launches the configured command.
- `configure_backend_mcp` now has an explicit `AgentKind::Other(_)` arm that uses `kind_from_command(&spec.program)` to detect the dialect and route through the matching wiring — so a custom agent `claude-yolo = "claude --dangerously-skip-permissions"` gets `attach_agent_mcp` (ClaudeCode wiring), and `my-agy = "agy --mode plan"` gets `ensure_antigravity_mcp_registration`.
- `AgentKind::Aider` now has an explicit no-op arm (documented, not silent fall-through).
- `AgentKind::ClaudeCode | AgentKind::Codex` now has an explicit no-op arm (with comment explaining that `attach_agent_mcp` handles them at the call site).

## Custom/Other agent fleet mail wiring

Custom agents defined in `config.toml` are resolved with `kind_from_command(command)` at spawn time (since A1), so their `AgentKind` is inferred from the binary name before flags are applied. This means:

```toml
[agents]
claude-yolo = "claude --dangerously-skip-permissions"   # → ClaudeCode wiring
my-agy      = "agy --mode plan"                         # → Antigravity wiring
my-cursor   = "cursor-agent --approve-mcps"             # → Cursor wiring
exotic-tool = "my-exotic-agent"                         # → no MCP wiring (unknown)
```

For `agent.adopt`, the kind arrives as `AgentKind::from_kind_str(p.agent)` (the custom name), which becomes `Other(name)`. The `configure_backend_mcp` `Other` arm then re-applies `kind_from_command(&spec.program)` to the actual executable in the spawn spec to recover the dialect.

## Aider MCP research findings

Probed & documented 2026-08-16:
- **Aider CLI MCP support**: No native MCP client support in the core Aider CLI as of mid-2025. `aider --help` has no `--mcp` flag. Community workarounds exist (`mcpm-aider`, `AiderDesk`) but require third-party wrappers and cannot be wired by repomon without knowing the specific wrapper.
- **Fleet mail**: Not available for Aider workers. The identity token and socket are still passed in the process environment in case a future version adds support. `configure_backend_mcp` is a documented no-op for `AgentKind::Aider`.

## Cursor CLI & MCP research findings

Probed & documented 2026-08-16:
- **CLI Binary**: `cursor-agent`
- **MCP Configuration**:
  - Global: `~/.cursor/mcp.json`
  - Project-level: `.cursor/mcp.json`
  - Standard `mcpServers` JSON structure:
    ```json
    {
      "mcpServers": {
        "repomon": {
          "command": "/path/to/repomond",
          "args": ["mcp"]
        }
      }
    }
    ```
- **CLI Commands & Flags**:
  - `cursor-agent <prompt>` (interactive)
  - `cursor-agent -p <prompt>` (headless/print)
  - `--approve-mcps` (auto-approve MCP tools in non-interactive / headless workflows)
  - `cursor-agent mcp list` / `cursor-agent mcp list-tools <id>`
- **Secrets-on-disk & Environment Isolation**:
  - Like Antigravity, Cursor registers the command executable and arguments in `~/.cursor/mcp.json` (or `REPOMON_CURSOR_MCP_CONFIG` in tests).
  - No secrets or identity tokens are written to disk; `repomon` supplies `REPOMON_MCP_SOCKET`, `REPOMON_MCP_MODE=agent`, and `REPOMON_MCP_IDENTITY_TOKEN` through the process environment at spawn/adopt time.
  - Spawned stdio MCP child processes inherit the process environment, maintaining security and token isolation.

## What works (kind-agnostic)

1. **Fleet Mail (`message_send` / `message_inbox` / `message_mark_read` / `fleet_status`)**:
   Fully kind-agnostic — addressed by `lane-X/slot`, not by kind. Any agent wired with the restricted
   MCP surface can mail any other agent regardless of kind. Supported for Claude, Codex, Antigravity,
   OpenCode, Cursor, and dialect-matching custom agents.
2. **Underlying Daemon RPCs (`agent.spawn`, `agent.send_input`, `agent.capture`, `agent.adopt`, `agent.stop`, etc.)**:
   Fully kind-agnostic — operates on lane/window addressing regardless of kind. All kinds are now adoptable.
3. **Orchestrator Allowlist**:
   `resolve_orchestrator_backend` supports Claude accounts, Codex, Antigravity, OpenCode, and user-defined custom commands.
