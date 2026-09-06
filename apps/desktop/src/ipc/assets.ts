import { invoke } from "@tauri-apps/api/core";

// Cache asset-protocol grants per worktree root to avoid repeating them for each opened file.
const allowedRoots = new Set<string>();

/// Grants the asset protocol read access to `worktreeRoot` the first time it is seen. A failed
/// grant is not cached, so a later retry (e.g. after a transient error) can still succeed.
export async function ensureWorktreeAssetsAllowed(worktreeRoot: string): Promise<void> {
  if (allowedRoots.has(worktreeRoot)) return;
  await invoke("allow_worktree_assets", { path: worktreeRoot });
  allowedRoots.add(worktreeRoot);
}

// Test-only: clears the module-level cache between test cases.
export function resetWorktreeAssetsAllowedCacheForTests(): void {
  allowedRoots.clear();
}
