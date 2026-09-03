import { createEffect, createSignal, onMount, Show } from "solid-js";
import { daemonCall } from "../ipc/rpc";
import { translateError, type TranslatedError } from "../ipc/errors";

export interface ImageViewerProps {
  laneId: number;
  path: string;
  size?: number;
}

function formatBytes(bytes?: number): string {
  if (bytes === undefined || bytes === null) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function basename(path: string): string {
  return path.split("/").pop() || path;
}

export default function ImageViewer(props: ImageViewerProps) {
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<TranslatedError | null>(null);
  const [dataUri, setDataUri] = createSignal<string | null>(null);
  const [dimensions, setDimensions] = createSignal<{ width: number; height: number } | null>(null);
  const [zoomToFit, setZoomToFit] = createSignal(true);
  const [fileSize, setFileSize] = createSignal<number>(props.size ?? 0);

  async function load() {
    setLoading(true);
    setError(null);
    setDataUri(null);
    try {
      const res = await daemonCall("file.read_raw", { lane_id: props.laneId, path: props.path });
      setDataUri(`data:${res.mime};base64,${res.base64}`);
      setFileSize(res.size);
      setLoading(false);
    } catch (cause) {
      setError(translateError(cause));
      setLoading(false);
    }
  }

  onMount(() => {
    void load();
  });

  createEffect(() => {
    // Re-load if path or laneId changes
    void props.path;
    void props.laneId;
    void load();
  });

  return (
    <div class="flex h-full flex-col bg-surface select-none">
      {/* Top info bar */}
      <div class="flex h-9 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3">
        <div class="flex items-center gap-2 font-mono text-[11px] text-muted">
          <span class="font-medium text-foreground">{basename(props.path)}</span>
          <Show when={dimensions()} keyed>
            {(dims) => (
              <>
                <span class="text-line">|</span>
                <span>
                  {dims.width} × {dims.height} px
                </span>
              </>
            )}
          </Show>
          <Show when={fileSize() > 0}>
            <span class="text-line">|</span>
            <span>{formatBytes(fileSize())}</span>
          </Show>
        </div>
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class={`focus-ring rounded border px-2 py-0.5 text-[11px] font-medium transition-colors ${
              zoomToFit()
                ? "border-accent/40 bg-accent/10 text-accent"
                : "border-line bg-surface text-muted hover:text-foreground"
            }`}
            onClick={() => setZoomToFit((v) => !v)}
          >
            {zoomToFit() ? "Fit" : "100%"}
          </button>
        </div>
      </div>

      {/* Image container */}
      <div
        class="relative flex min-h-0 flex-1 items-center justify-center overflow-auto p-4"
        style={{
          "background-image":
            "radial-gradient(var(--line) 1px, transparent 1px), radial-gradient(var(--line) 1px, var(--surface) 1px)",
          "background-size": "16px 16px",
          "background-position": "0 0, 8px 8px",
        }}
      >
        <Show when={loading()}>
          <p class="font-mono text-xs text-muted">Loading image...</p>
        </Show>

        <Show when={error()} keyed>
          {(err) => (
            <div class="max-w-xs space-y-1 text-center">
              <p class="text-xs font-medium text-fault">Failed to load image</p>
              <p class="text-xs text-muted">{err.friendly}</p>
            </div>
          )}
        </Show>

        <Show when={dataUri()} keyed>
          {(uri) => (
            <img
              src={uri}
              alt={basename(props.path)}
              class={`rounded shadow-md transition-all ${
                zoomToFit() ? "max-h-full max-w-full object-contain" : "h-auto w-auto max-w-none"
              }`}
              onLoad={(e) => {
                const img = e.currentTarget;
                setDimensions({ width: img.naturalWidth, height: img.naturalHeight });
              }}
            />
          )}
        </Show>
      </div>
    </div>
  );
}
