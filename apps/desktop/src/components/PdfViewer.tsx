import { Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { openPath } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";

import { ensureWorktreeAssetsAllowed } from "../ipc/assets";

export interface PdfViewerProps {
  worktreeRoot: string;
  path: string;
  size?: number;
}

// How long an iframe is given to fire its `load` event before the preview is treated as failed.
// The webview's own PDF renderer paints fast once the asset request lands; a load that is still
// pending past this is a renderer that isn't going to come up (an unsupported build, a scope
// grant that silently failed) rather than one that's merely slow.
const LOAD_TIMEOUT_MS = 5000;

type PdfStatus = "granting" | "loading" | "loaded" | "failed";

function formatBytes(bytes?: number): string {
  if (bytes === undefined || bytes === null) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function basename(path: string): string {
  return path.split("/").pop() || path;
}

// WebKitGTK (the Linux webview) has no built-in PDF renderer, unlike WebKit on macOS and
// WebView2 on Windows - an iframe pointed at a PDF there just shows a download prompt or a blank
// pane, so Linux skips the iframe entirely rather than rendering something broken.
export function isLinuxWebview(userAgent: string): boolean {
  return userAgent.includes("Linux") && !userAgent.includes("Android");
}

export default function PdfViewer(props: PdfViewerProps) {
  const linux = isLinuxWebview(navigator.userAgent);
  const [status, setStatus] = createSignal<PdfStatus>(linux ? "failed" : "granting");
  const absolutePath = createMemo(() => `${props.worktreeRoot}/${props.path}`);
  const assetUrl = createMemo(() => convertFileSrc(absolutePath()));

  let timeoutId: ReturnType<typeof setTimeout> | undefined;

  function clearLoadTimeout() {
    if (timeoutId !== undefined) {
      clearTimeout(timeoutId);
      timeoutId = undefined;
    }
  }

  async function load() {
    clearLoadTimeout();
    if (linux) {
      setStatus("failed");
      return;
    }
    setStatus("granting");
    try {
      await ensureWorktreeAssetsAllowed(props.worktreeRoot);
    } catch {
      setStatus("failed");
      return;
    }
    setStatus("loading");
    timeoutId = setTimeout(() => {
      // Only a still-pending load times out - a load that already settled (either way) has
      // already cleared this timer.
      setStatus((current) => (current === "loading" ? "failed" : current));
    }, LOAD_TIMEOUT_MS);
  }

  createEffect(() => {
    // Re-run whenever the file identity changes.
    void props.worktreeRoot;
    void props.path;
    void load();
  });

  onCleanup(clearLoadTimeout);

  async function openExternally() {
    try {
      await openPath(absolutePath());
    } catch (err) {
      console.warn("openPath failed:", err);
    }
  }

  // The open-externally button is the app's normal, secondary way to hand a PDF to the system
  // viewer while the in-app preview is working. Once that preview can't be shown - no renderer on
  // this platform, or a load that failed or timed out - it becomes the only way to view the file
  // at all, so it takes over as the toolbar's primary action.
  const isOnlyWayToView = () => status() === "failed";

  return (
    <div class="flex h-full flex-col bg-surface select-none">
      <div class="flex h-9 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3">
        <div class="flex min-w-0 items-center gap-2 font-mono text-[11px] text-muted">
          <span class="truncate font-medium text-foreground">{basename(props.path)}</span>
          <Show when={props.size !== undefined && props.size > 0}>
            <span class="shrink-0 text-line">|</span>
            <span class="shrink-0">{formatBytes(props.size)}</span>
          </Show>
        </div>
        <button
          type="button"
          class={
            isOnlyWayToView()
              ? "focus-ring shrink-0 rounded bg-signal px-2.5 py-1 text-[11px] font-semibold text-background transition-colors hover:bg-signal/90"
              : "focus-ring shrink-0 rounded border border-line bg-surface px-2 py-0.5 text-[11px] font-medium text-muted transition-colors hover:border-line/80 hover:text-foreground"
          }
          onClick={() => void openExternally()}
        >
          Open in system viewer
        </button>
      </div>

      <div class="relative min-h-0 flex-1 overflow-hidden bg-surface">
        <Show when={status() === "granting" || status() === "loading"}>
          <div class="absolute inset-0 flex items-center justify-center">
            <p class="font-mono text-xs text-muted">Loading preview...</p>
          </div>
        </Show>

        {/* Mounting is gated on "loading"/"loaded", not merely "not failed": mounting the
            iframe is what makes the webview issue the actual asset request, and that request
            must not go out before the Rust side has granted the asset-protocol scope for this
            worktree root (see `load` above) - otherwise it fails the race and errors out on a
            file that was never actually unreadable. */}
        <Show when={!linux && (status() === "loading" || status() === "loaded")}>
          <iframe
            src={assetUrl()}
            title={basename(props.path)}
            class="h-full w-full border-0"
            onLoad={() => {
              clearLoadTimeout();
              setStatus("loaded");
            }}
            onError={() => {
              clearLoadTimeout();
              setStatus("failed");
            }}
          />
        </Show>

        <Show when={status() === "failed"}>
          <div class="flex h-full flex-col items-center justify-center gap-1.5 p-6 text-center">
            <p class={`text-xs font-medium ${linux ? "text-foreground" : "text-fault"}`}>
              {linux ? "Preview unavailable" : "Couldn't load preview"}
            </p>
            <p class="max-w-xs text-xs text-muted">
              {linux
                ? "PDF preview is not available on Linux. Use Open in system viewer above."
                : "This PDF could not be displayed here. Use Open in system viewer above."}
            </p>
          </div>
        </Show>
      </div>
    </div>
  );
}
