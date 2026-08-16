# Cross-kind agent orchestration: architecture & limitations

Updated 2026-08-16 (Track A: cross-kind orchestration).

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

## What works (kind-agnostic)

1. **Fleet Mail (`message_send` / `message_inbox` / `message_mark_read` / `fleet_status`)**:
   Fully kind-agnostic — addressed by `lane-X/slot`, not by kind. Any agent wired with the restricted
   MCP surface can mail any other agent regardless of kind. Supported for Claude, Codex, Antigravity,
   OpenCode, and Cursor.
2. **Underlying Daemon RPCs (`agent.spawn`, `agent.send_input`, `agent.capture`, `agent.adopt`, `agent.stop`, etc.)**:
   Fully kind-agnostic — operates on lane/window addressing regardless of kind.
3. **Orchestrator Allowlist**:
   `resolve_orchestrator_backend` supports Claude accounts, Codex, Antigravity, OpenCode, and user-defined custom commands.

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
