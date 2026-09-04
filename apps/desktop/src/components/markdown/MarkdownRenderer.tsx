import {
  createEffect,
  createSignal,
  For,
  onCleanup,
  Show,
  type JSX,
} from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { daemonCall } from "../../ipc/rpc";
import MiniCodeBlock from "./MiniCodeBlock";
import {
  type Alignment,
  type BlockNode,
  type HeadingNode,
  type InlineNode,
  type ListItemNode,
  type ListNode,
  type TableNode,
} from "./parser";

export interface MarkdownRendererProps {
  ast: BlockNode[];
  filePath?: string;
  laneId?: number;
  onNavigateHeading?: (slug: string) => void;
}

export function resolveRelativePath(baseFile: string, relativePath: string): string {
  if (relativePath.startsWith("/")) {
    return relativePath.replace(/^\/+/, "");
  }
  const dirParts = baseFile.split("/").slice(0, -1);
  const relParts = relativePath.split("/");
  for (const part of relParts) {
    if (part === "." || part === "") continue;
    if (part === "..") {
      dirParts.pop();
    } else {
      dirParts.push(part);
    }
  }
  return dirParts.join("/");
}

function LocalImage(props: {
  src: string;
  alt: string;
  title?: string;
  baseFile?: string;
  laneId?: number;
}) {
  const [dataUri, setDataUri] = createSignal<string | null>(null);
  const [error, setError] = createSignal(false);
  const [loading, setLoading] = createSignal(true);
  let requestToken = 0;

  createEffect(() => {
    const src = props.src;
    const laneId = props.laneId;
    const baseFile = props.baseFile;

    // External or data URLs can be used directly
    if (
      src.startsWith("http://") ||
      src.startsWith("https://") ||
      src.startsWith("data:") ||
      src.startsWith("blob:")
    ) {
      setDataUri(src);
      setLoading(false);
      setError(false);
      return;
    }

    if (laneId === undefined || !baseFile) {
      setError(true);
      setLoading(false);
      return;
    }

    const token = ++requestToken;
    setLoading(true);
    setError(false);

    const resolved = resolveRelativePath(baseFile, src);
    daemonCall("file.read_raw", { lane_id: laneId, path: resolved })
      .then((res) => {
        if (token !== requestToken) return;
        setDataUri(`data:${res.mime};base64,${res.base64}`);
        setLoading(false);
      })
      .catch(() => {
        if (token !== requestToken) return;
        setError(true);
        setLoading(false);
      });
  });

  onCleanup(() => {
    requestToken++;
  });

  return (
    <span class="my-2 inline-block max-w-full">
      <Show
        when={!loading() && dataUri() && !error()}
        fallback={
          <span class="inline-flex items-center gap-1.5 rounded border border-line bg-raised/30 px-2 py-1 text-xs text-muted">
            <svg
              class="size-3.5 shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
              <circle cx="9" cy="9" r="2" />
              <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
            </svg>
            <span>{props.alt || props.src}</span>
            <Show when={error()}>
              <span class="text-fault font-mono text-[10px]">(failed to load)</span>
            </Show>
          </span>
        }
      >
        <img
          src={dataUri()!}
          alt={props.alt}
          title={props.title}
          class="max-h-[500px] max-w-full rounded border border-line object-contain"
        />
      </Show>
    </span>
  );
}

function RenderInline(props: {
  node: InlineNode;
  filePath?: string;
  laneId?: number;
  onNavigateHeading?: (slug: string) => void;
}): JSX.Element {
  const node = props.node;

  switch (node.type) {
    case "text":
      return <>{node.text}</>;

    case "bold":
      return (
        <strong class="font-semibold text-foreground">
          <For each={node.children}>
            {(child) => (
              <RenderInline
                node={child}
                filePath={props.filePath}
                laneId={props.laneId}
                onNavigateHeading={props.onNavigateHeading}
              />
            )}
          </For>
        </strong>
      );

    case "italic":
      return (
        <em class="italic text-foreground/95">
          <For each={node.children}>
            {(child) => (
              <RenderInline
                node={child}
                filePath={props.filePath}
                laneId={props.laneId}
                onNavigateHeading={props.onNavigateHeading}
              />
            )}
          </For>
        </em>
      );

    case "boldItalic":
      return (
        <strong class="font-semibold italic text-foreground">
          <For each={node.children}>
            {(child) => (
              <RenderInline
                node={child}
                filePath={props.filePath}
                laneId={props.laneId}
                onNavigateHeading={props.onNavigateHeading}
              />
            )}
          </For>
        </strong>
      );

    case "strikethrough":
      return (
        <del class="line-through text-muted">
          <For each={node.children}>
            {(child) => (
              <RenderInline
                node={child}
                filePath={props.filePath}
                laneId={props.laneId}
                onNavigateHeading={props.onNavigateHeading}
              />
            )}
          </For>
        </del>
      );

    case "inlineCode":
      return (
        <code class="rounded border border-line/60 bg-raised/60 px-1 py-0.5 font-mono text-[11px] text-accent">
          {node.code}
        </code>
      );

    case "link": {
      const handleClick = (e: MouseEvent) => {
        e.preventDefault();
        const url = node.url;
        if (!url || url === "#") return;

        if (url.startsWith("#")) {
          const targetSlug = url.slice(1);
          props.onNavigateHeading?.(targetSlug);
          const el = document.getElementById(targetSlug);
          if (el) {
            el.scrollIntoView({ behavior: "smooth", block: "start" });
          }
          return;
        }

        void Promise.resolve(openUrl(url)).catch(() => {
          // Ignore external opener failures
        });
      };

      return (
        <a
          href={node.url}
          title={node.title}
          onClick={handleClick}
          class="cursor-pointer text-accent underline decoration-accent/40 underline-offset-2 transition-colors hover:decoration-accent"
        >
          <For each={node.children}>
            {(child) => (
              <RenderInline
                node={child}
                filePath={props.filePath}
                laneId={props.laneId}
                onNavigateHeading={props.onNavigateHeading}
              />
            )}
          </For>
        </a>
      );
    }

    case "image":
      return (
        <LocalImage
          src={node.src}
          alt={node.alt}
          title={node.title}
          baseFile={props.filePath}
          laneId={props.laneId}
        />
      );

    default:
      return null;
  }
}

function RenderHeading(props: {
  node: HeadingNode;
  filePath?: string;
  laneId?: number;
  onNavigateHeading?: (slug: string) => void;
}) {
  const node = props.node;

  const inner = (
    <For each={node.children}>
      {(child) => (
        <RenderInline
          node={child}
          filePath={props.filePath}
          laneId={props.laneId}
          onNavigateHeading={props.onNavigateHeading}
        />
      )}
    </For>
  );

  switch (node.level) {
    case 1:
      return (
        <h1
          id={node.slug}
          data-heading-line={node.line}
          class="mb-4 mt-6 border-b border-line pb-2 text-2xl font-bold tracking-tight text-foreground"
        >
          {inner}
        </h1>
      );
    case 2:
      return (
        <h2
          id={node.slug}
          data-heading-line={node.line}
          class="mb-3 mt-5 border-b border-line/60 pb-1.5 text-xl font-semibold tracking-tight text-foreground"
        >
          {inner}
        </h2>
      );
    case 3:
      return (
        <h3
          id={node.slug}
          data-heading-line={node.line}
          class="mb-2 mt-4 text-lg font-semibold text-foreground"
        >
          {inner}
        </h3>
      );
    case 4:
      return (
        <h4
          id={node.slug}
          data-heading-line={node.line}
          class="mb-2 mt-3 text-base font-medium text-foreground"
        >
          {inner}
        </h4>
      );
    case 5:
      return (
        <h5
          id={node.slug}
          data-heading-line={node.line}
          class="mb-1 mt-2 text-sm font-medium text-foreground"
        >
          {inner}
        </h5>
      );
    case 6:
      return (
        <h6
          id={node.slug}
          data-heading-line={node.line}
          class="mb-1 mt-2 text-xs font-medium uppercase tracking-wider text-muted"
        >
          {inner}
        </h6>
      );
  }
}

function RenderList(props: {
  node: ListNode;
  filePath?: string;
  laneId?: number;
  onNavigateHeading?: (slug: string) => void;
}) {
  const isOrdered = props.node.ordered;

  return (
    <Show
      when={isOrdered}
      fallback={
        <ul class="my-3 ml-6 list-disc space-y-1">
          <For each={props.node.items}>
            {(item) => (
              <RenderListItem
                item={item}
                filePath={props.filePath}
                laneId={props.laneId}
                onNavigateHeading={props.onNavigateHeading}
              />
            )}
          </For>
        </ul>
      }
    >
      <ol
        start={props.node.start ?? 1}
        class="my-3 ml-6 list-decimal space-y-1"
      >
        <For each={props.node.items}>
          {(item) => (
            <RenderListItem
              item={item}
              filePath={props.filePath}
              laneId={props.laneId}
              onNavigateHeading={props.onNavigateHeading}
            />
          )}
        </For>
      </ol>
    </Show>
  );
}

function RenderListItem(props: {
  item: ListItemNode;
  filePath?: string;
  laneId?: number;
  onNavigateHeading?: (slug: string) => void;
}) {
  const isTask = props.item.task !== undefined;

  return (
    <li
      class={
        isTask
          ? "flex list-none items-start gap-2 -ml-5 text-foreground/90"
          : "text-foreground/90 leading-relaxed"
      }
    >
      <Show when={isTask}>
        <input
          type="checkbox"
          checked={props.item.task?.checked}
          disabled
          class="mt-1 size-3.5 rounded border border-line accent-accent"
        />
      </Show>
      <div class="min-w-0 flex-1">
        <For each={props.item.children}>
          {(child) => (
            <RenderInline
              node={child}
              filePath={props.filePath}
              laneId={props.laneId}
              onNavigateHeading={props.onNavigateHeading}
            />
          )}
        </For>
        <Show when={props.item.subList}>
          <RenderList
            node={props.item.subList!}
            filePath={props.filePath}
            laneId={props.laneId}
            onNavigateHeading={props.onNavigateHeading}
          />
        </Show>
      </div>
    </li>
  );
}

function RenderTable(props: {
  node: TableNode;
  filePath?: string;
  laneId?: number;
  onNavigateHeading?: (slug: string) => void;
}) {
  const getAlignClass = (align: Alignment) => {
    if (align === "center") return "text-center";
    if (align === "right") return "text-right";
    return "text-left";
  };

  return (
    <div class="my-4 max-w-full overflow-x-auto rounded-md border border-line">
      <table class="w-full border-collapse text-xs">
        <thead class="border-b border-line bg-surface/80">
          <tr>
            <For each={props.node.headers}>
              {(header, idx) => (
                <th
                  class={`px-3 py-2 font-semibold text-foreground ${getAlignClass(
                    props.node.alignments[idx()]
                  )}`}
                >
                  <For each={header}>
                    {(child) => (
                      <RenderInline
                        node={child}
                        filePath={props.filePath}
                        laneId={props.laneId}
                        onNavigateHeading={props.onNavigateHeading}
                      />
                    )}
                  </For>
                </th>
              )}
            </For>
          </tr>
        </thead>
        <tbody class="divide-y divide-line/40">
          <For each={props.node.rows}>
            {(row) => (
              <tr class="hover:bg-raised/20">
                <For each={row}>
                  {(cell, idx) => (
                    <td
                      class={`px-3 py-2 text-foreground/90 ${getAlignClass(
                        props.node.alignments[idx()]
                      )}`}
                    >
                      <For each={cell}>
                        {(child) => (
                          <RenderInline
                            node={child}
                            filePath={props.filePath}
                            laneId={props.laneId}
                            onNavigateHeading={props.onNavigateHeading}
                          />
                        )}
                      </For>
                    </td>
                  )}
                </For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

export default function MarkdownRenderer(props: MarkdownRendererProps) {
  return (
    <div class="markdown-body space-y-3 font-sans text-sm text-foreground/90 leading-relaxed">
      <For each={props.ast}>
        {(block) => {
          switch (block.type) {
            case "heading":
              return (
                <RenderHeading
                  node={block}
                  filePath={props.filePath}
                  laneId={props.laneId}
                  onNavigateHeading={props.onNavigateHeading}
                />
              );

            case "paragraph":
              return (
                <p class="leading-relaxed text-foreground/90">
                  <For each={block.children}>
                    {(child) => (
                      <RenderInline
                        node={child}
                        filePath={props.filePath}
                        laneId={props.laneId}
                        onNavigateHeading={props.onNavigateHeading}
                      />
                    )}
                  </For>
                </p>
              );

            case "blockquote":
              return (
                <blockquote class="my-4 border-l-2 border-accent/60 pl-4 italic text-muted">
                  <MarkdownRenderer
                    ast={block.children}
                    filePath={props.filePath}
                    laneId={props.laneId}
                    onNavigateHeading={props.onNavigateHeading}
                  />
                </blockquote>
              );

            case "codeBlock":
              return (
                <MiniCodeBlock
                  code={block.code}
                  language={block.language}
                />
              );

            case "list":
              return (
                <RenderList
                  node={block}
                  filePath={props.filePath}
                  laneId={props.laneId}
                  onNavigateHeading={props.onNavigateHeading}
                />
              );

            case "table":
              return (
                <RenderTable
                  node={block}
                  filePath={props.filePath}
                  laneId={props.laneId}
                  onNavigateHeading={props.onNavigateHeading}
                />
              );

            case "thematicBreak":
              return <hr class="my-6 border-line" />;

            default:
              return null;
          }
        }}
      </For>
    </div>
  );
}
