import type { ConversationRow } from "./transcript";

/// The last rendered page for a given watch identity, kept outside any one `ConversationPane`
/// mount so a lane that already painted once (this session) repaints instantly the next time its
/// pane mounts, instead of a blank "Opening conversation…" wait while the watch re-attaches.
/// Keyed by the exact watch params (the same identity `createTranscript` already uses to decide
/// whether it retained state), so a fresh identity - a different lane, window, session, or kind -
/// never reads another identity's entry; a stale or mismatched session can't leak another agent's
/// history into this window's first paint.
export interface CachedTranscriptPage {
  rows: ConversationRow[];
  nextBefore: number | null;
  remaining: number | null;
}

const pages = new Map<string, CachedTranscriptPage>();

export function getCachedTranscriptPage(key: string): CachedTranscriptPage | undefined {
  return pages.get(key);
}

export function setCachedTranscriptPage(key: string, page: CachedTranscriptPage): void {
  pages.set(key, page);
}

// Test-only: clears every cached page between test cases.
export function resetTranscriptCacheForTests(): void {
  pages.clear();
}
