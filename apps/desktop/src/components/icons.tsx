import { Match, Switch, createSignal, type JSX } from "solid-js";

export interface IconProps {
  size?: number;
  class?: string;
  strokeWidth?: number;
}

export function IconSearch(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="11" cy="11" r="7" />
      <line x1="21" y1="21" x2="16.65" y2="16.65" />
    </svg>
  );
}

export function IconPlus(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}

export function IconClose(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="18" y1="6" x2="6" y2="18" />
      <line x1="6" y1="6" x2="18" y2="18" />
    </svg>
  );
}

export function IconHide(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <line x1="5.6" y1="5.6" x2="18.4" y2="18.4" />
    </svg>
  );
}

export function IconTrash(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="3 6 5 6 21 6" />
      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
    </svg>
  );
}

export function IconPin(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="12" y1="17" x2="12" y2="22" />
      <path d="M5 17h14l-2-7V4a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v6L5 17z" />
    </svg>
  );
}

export function IconGitBranch(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="6" y1="3" x2="6" y2="15" />
      <circle cx="18" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <path d="M18 9a9 9 0 0 1-9 9" />
    </svg>
  );
}

export function IconArrowUp(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 2}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="12" y1="19" x2="12" y2="5" />
      <polyline points="5 12 12 5 19 12" />
    </svg>
  );
}

export function IconArrowDown(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 2}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="12" y1="5" x2="12" y2="19" />
      <polyline points="19 12 12 19 5 12" />
    </svg>
  );
}

export function IconTerminal(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="4 17 10 11 4 5" />
      <line x1="12" y1="19" x2="20" y2="19" />
    </svg>
  );
}

export function IconBot(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <rect x="3" y="11" width="18" height="10" rx="2" />
      <circle cx="12" cy="5" r="2" />
      <path d="M12 7v4" />
      <line x1="8" y1="16" x2="8.01" y2="16" />
      <line x1="16" y1="16" x2="16.01" y2="16" />
    </svg>
  );
}

export function IconSettings(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

export function IconExtensions(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M20.5 13.5a2.5 2.5 0 0 1-2.5-2.5V8a2 2 0 0 0-2-2h-3a2.5 2.5 0 0 1-5 0H5a2 2 0 0 0-2 2v3a2.5 2.5 0 0 1 0 5v3a2 2 0 0 0 2 2h3a2.5 2.5 0 0 1 5 0h3a2 2 0 0 0 2-2v-3a2.5 2.5 0 0 1 2.5-2.5z" />
    </svg>
  );
}

export function IconSparkles(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M12 3l1.9 5.8a2 2 0 0 0 1.3 1.3L21 12l-5.8 1.9a2 2 0 0 0-1.3 1.3L12 21l-1.9-5.8a2 2 0 0 0-1.3-1.3L3 12l5.8-1.9a2 2 0 0 0 1.3-1.3L12 3z" />
    </svg>
  );
}

export function IconCommand(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M18 3a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3 3 3 0 0 0 3-3 3 3 0 0 0-3-3H6a3 3 0 0 0-3 3 3 3 0 0 0 3 3 3 3 0 0 0 3-3V6a3 3 0 0 0-3-3 3 3 0 0 0-3 3 3 3 0 0 0 3 3h12a3 3 0 0 0 3-3 3 3 0 0 0-3-3z" />
    </svg>
  );
}

export function IconRefresh(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="23 4 23 10 17 10" />
      <polyline points="1 20 1 14 7 14" />
      <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
    </svg>
  );
}

export function IconPlay(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      class={props.class}
      aria-hidden="true"
    >
      <polygon points="5 3 19 12 5 21 5 3" />
    </svg>
  );
}

export function IconStop(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      class={props.class}
      aria-hidden="true"
    >
      <rect x="4" y="4" width="16" height="16" rx="2" />
    </svg>
  );
}

export function IconChevronDown(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 2}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="6 9 12 15 18 9" />
    </svg>
  );
}

export function IconChevronRight(props: IconProps): JSX.Element {
  const s = () => props.size ?? 12;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 2}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="9 18 15 12 9 6" />
    </svg>
  );
}

export function IconCheck(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 2}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="20 6 9 17 4 12" />
    </svg>
  );
}

export function IconLayers(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polygon points="12 2 2 7 12 12 22 7 12 2" />
      <polyline points="2 17 12 22 22 17" />
      <polyline points="2 12 12 17 22 12" />
    </svg>
  );
}

export function IconSplit(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <line x1="12" y1="3" x2="12" y2="21" />
    </svg>
  );
}

export function IconGrid(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <rect x="3" y="3" width="7" height="7" rx="1" />
      <rect x="14" y="3" width="7" height="7" rx="1" />
      <rect x="14" y="14" width="7" height="7" rx="1" />
      <rect x="3" y="14" width="7" height="7" rx="1" />
    </svg>
  );
}

export function IconFocus(props: IconProps): JSX.Element {
  const s = () => props.size ?? 13;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <rect x="3" y="3" width="18" height="18" rx="2" />
    </svg>
  );
}

export function IconAgentClaudeCode(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="12" y1="2" x2="12" y2="22" />
      <line x1="2" y1="12" x2="22" y2="12" />
      <line x1="4.93" y1="4.93" x2="19.07" y2="19.07" />
      <line x1="19.07" y1="4.93" x2="4.93" y2="19.07" />
      <circle cx="12" cy="12" r="2.5" fill="currentColor" />
    </svg>
  );
}

export function IconAgentCursor(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polygon points="4 4 11 20 14 13 21 10 4 4" />
      <line x1="13.5" y1="13.5" x2="19" y2="19" />
    </svg>
  );
}

export function IconAgentAider(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="6" cy="12" r="3" />
      <circle cx="18" cy="12" r="3" />
      <path d="M9 12h6" />
      <path d="M12 9l3 3-3 3" />
      <path d="M6 9a6 6 0 0 1 12 0" stroke-dasharray="2 2" />
    </svg>
  );
}

export function IconAgentCodex(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="7 8 3 12 7 16" />
      <polyline points="17 8 21 12 17 16" />
      <line x1="14" y1="4" x2="10" y2="20" />
    </svg>
  );
}

export function IconAgentAntigravity(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M12 3L4 17h16L12 3z" />
      <circle cx="12" cy="12" r="2" fill="currentColor" />
      <ellipse cx="12" cy="17" rx="9" ry="3" stroke-dasharray="2 2" />
    </svg>
  );
}

export function IconAgentOpenCode(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M7 4a3 3 0 0 0-3 3v2a3 3 0 0 1-3 3 3 3 0 0 1 3 3v2a3 3 0 0 0 3 3" />
      <path d="M17 4a3 3 0 0 1 3 3v2a3 3 0 0 0 3 3 3 3 0 0 0-3 3v2a3 3 0 0 1-3 3" />
      <polyline points="10 9 13 12 10 15" />
    </svg>
  );
}

export function IconBolt(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
    </svg>
  );
}

export function IconCompass(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <polygon points="12 6 15 12 12 18 9 12 12 6" fill="currentColor" fill-opacity="0.18" />
      <circle cx="12" cy="12" r="1.5" fill="currentColor" />
    </svg>
  );
}

export function IconCube(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M12 2L20 6.5V15.5L12 20L4 15.5V6.5L12 2Z" />
      <path d="M12 2V11L20 6.5" />
      <path d="M12 11L4 6.5" />
      <path d="M12 11V20" />
    </svg>
  );
}

export function IconDna(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M4 6C8 6 8 18 12 18S16 6 20 6" />
      <path d="M4 18C8 18 8 6 12 6S16 18 20 18" />
      <circle cx="4" cy="6" r="1.5" fill="currentColor" />
      <circle cx="20" cy="6" r="1.5" fill="currentColor" />
      <circle cx="12" cy="12" r="1.5" fill="currentColor" />
    </svg>
  );
}

export function IconRadar(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="5" />
      <circle cx="12" cy="12" r="1.5" fill="currentColor" />
      <line x1="12" y1="12" x2="18.5" y2="5.5" />
    </svg>
  );
}

export function IconShield(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M12 22S20 18 20 12V5L12 2L4 5V12C4 18 12 22 12 22Z" />
      <polyline points="9 12 11 14 15 10" />
    </svg>
  );
}

export function IconCpu(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <rect x="5" y="5" width="14" height="14" rx="2" />
      <rect x="9" y="9" width="6" height="6" />
      <line x1="9" y1="1" x2="9" y2="5" />
      <line x1="15" y1="1" x2="15" y2="5" />
      <line x1="9" y1="19" x2="9" y2="23" />
      <line x1="15" y1="19" x2="15" y2="23" />
      <line x1="1" y1="9" x2="5" y2="9" />
      <line x1="1" y1="15" x2="5" y2="15" />
      <line x1="19" y1="9" x2="23" y2="9" />
      <line x1="19" y1="15" x2="23" y2="15" />
    </svg>
  );
}

export function IconFlame(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M12 2C12 5 9 7 9 11A3 3 0 0 0 15 11C15 7 12 5 12 2Z" />
      <path d="M8.5 8C5.5 10.5 4 13.5 4 16A8 8 0 0 0 20 16C20 13.5 18.5 10.5 15.5 8" />
    </svg>
  );
}

export function IconOrbit(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="2.5" fill="currentColor" />
      <ellipse cx="12" cy="12" rx="9" ry="3.5" transform="rotate(-30 12 12)" />
      <ellipse cx="12" cy="12" rx="9" ry="3.5" transform="rotate(30 12 12)" />
    </svg>
  );
}

export function IconNetwork(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="4" r="2.5" />
      <circle cx="4" cy="19" r="2.5" />
      <circle cx="20" cy="19" r="2.5" />
      <line x1="12" y1="6.5" x2="4" y2="16.5" />
      <line x1="12" y1="6.5" x2="20" y2="16.5" />
      <line x1="6.5" y1="19" x2="17.5" y2="19" />
    </svg>
  );
}

export function IconWand(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <line x1="15" y1="4" x2="20" y2="9" />
      <line x1="4" y1="20" x2="17" y2="7" />
      <line x1="14.5" y1="6.5" x2="17.5" y2="9.5" />
      <path d="M8 2l.5 1.5L10 4l-1.5.5L8 6l-.5-1.5L6 4l1.5-.5L8 2z" fill="currentColor" />
    </svg>
  );
}

export function IconFeather(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M20.24 3.76A6 6 0 0 0 11.75 3.76L3 12.5V21H11.5L20.24 12.25A6 6 0 0 0 20.24 3.76Z" />
      <line x1="16" y1="8" x2="2" y2="22" />
      <line x1="17.5" y1="15" x2="9" y2="15" />
    </svg>
  );
}

export function IconAperture(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <line x1="14.31" y1="8" x2="20.05" y2="17.94" />
      <line x1="9.69" y1="8" x2="21.17" y2="8" />
      <line x1="7.38" y1="12" x2="13.12" y2="2.06" />
      <line x1="9.69" y1="16" x2="3.95" y2="6.06" />
      <line x1="14.31" y1="16" x2="2.83" y2="16" />
      <line x1="16.62" y1="12" x2="10.88" y2="21.94" />
    </svg>
  );
}

export function IconPulse(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polyline points="2 12 6 12 9 4 15 20 18 12 22 12" />
    </svg>
  );
}

export function IconGlobe(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <line x1="3.6" y1="9" x2="20.4" y2="9" />
      <line x1="3.6" y1="15" x2="20.4" y2="15" />
      <ellipse cx="12" cy="12" rx="4.5" ry="9" />
    </svg>
  );
}

export function IconZap(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.strokeWidth ?? 1.75}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
      <line x1="2" y1="2" x2="22" y2="22" stroke-dasharray="2 2" />
    </svg>
  );
}

export interface AgentIconCatalogEntry {
  id: string;
  label: string;
  category: "agents" | "system" | "creative" | "analysis";
  Icon: (props: IconProps) => JSX.Element;
}

export const AGENT_ICON_CATALOG: AgentIconCatalogEntry[] = [
  { id: "sparkle", label: "Sparkle Star", category: "agents", Icon: IconAgentClaudeCode },
  { id: "cursor", label: "Pointer Ray", category: "agents", Icon: IconAgentCursor },
  { id: "binary-orbit", label: "Binary Orbit", category: "agents", Icon: IconAgentAider },
  { id: "code-brackets", label: "Code Brackets", category: "agents", Icon: IconAgentCodex },
  { id: "antigravity", label: "Pyramid Ring", category: "agents", Icon: IconAgentAntigravity },
  { id: "brackets", label: "Curly Braces", category: "agents", Icon: IconAgentOpenCode },
  { id: "bot", label: "Robot Visor", category: "agents", Icon: IconBot },
  { id: "bolt", label: "Lightning Bolt", category: "system", Icon: IconBolt },
  { id: "compass", label: "Compass", category: "analysis", Icon: IconCompass },
  { id: "cube", label: "Isometric Cube", category: "creative", Icon: IconCube },
  { id: "dna", label: "Double Helix", category: "analysis", Icon: IconDna },
  { id: "radar", label: "Radar Sweep", category: "analysis", Icon: IconRadar },
  { id: "shield", label: "Security Shield", category: "system", Icon: IconShield },
  { id: "cpu", label: "Microchip", category: "system", Icon: IconCpu },
  { id: "flame", label: "Flame", category: "creative", Icon: IconFlame },
  { id: "layers", label: "Stack Layers", category: "creative", Icon: IconLayers },
  { id: "orbit", label: "Orbital Rings", category: "creative", Icon: IconOrbit },
  { id: "network", label: "Graph Network", category: "analysis", Icon: IconNetwork },
  { id: "sparkles", label: "Twin Stars", category: "creative", Icon: IconSparkles },
  { id: "wand", label: "Magic Stylus", category: "creative", Icon: IconWand },
  { id: "feather", label: "Quill Pen", category: "creative", Icon: IconFeather },
  { id: "aperture", label: "Aperture Lens", category: "analysis", Icon: IconAperture },
  { id: "terminal", label: "Command Terminal", category: "system", Icon: IconTerminal },
  { id: "pulse", label: "Activity Pulse", category: "analysis", Icon: IconPulse },
  { id: "globe", label: "World Grid", category: "analysis", Icon: IconGlobe },
  { id: "zap", label: "High Voltage", category: "system", Icon: IconZap },
];

export const [agentIconOverrides, setAgentIconOverrides] = createSignal<Record<string, string>>({});

export function resolveAgentIconKey(agent?: string | null, customKey?: string | null): string {
  if (customKey) return customKey;
  const raw = agent?.toLowerCase().trim() ?? "";
  if (!raw) return "bot";

  const overrides = agentIconOverrides();
  if (overrides[raw]) return overrides[raw];
  if (agent && overrides[agent]) return overrides[agent];

  if (raw === "claude-code" || raw === "claude") return "sparkle";
  if (raw === "cursor") return "cursor";
  if (raw === "aider") return "binary-orbit";
  if (raw === "codex") return "code-brackets";
  if (raw === "antigravity" || raw === "agy") return "antigravity";
  if (raw === "opencode") return "brackets";
  return "bot";
}

export interface AgentIconProps extends IconProps {
  agent?: string | null;
  iconKey?: string | null;
  shell?: boolean;
}

export function AgentIcon(props: AgentIconProps): JSX.Element {
  if (props.shell) {
    return <IconTerminal {...props} />;
  }

  const iconKey = () => resolveAgentIconKey(props.agent, props.iconKey);

  return (
    <Switch fallback={<IconBot {...props} />}>
      <Match when={iconKey() === "sparkle"}><IconAgentClaudeCode {...props} /></Match>
      <Match when={iconKey() === "cursor"}><IconAgentCursor {...props} /></Match>
      <Match when={iconKey() === "binary-orbit"}><IconAgentAider {...props} /></Match>
      <Match when={iconKey() === "code-brackets"}><IconAgentCodex {...props} /></Match>
      <Match when={iconKey() === "antigravity"}><IconAgentAntigravity {...props} /></Match>
      <Match when={iconKey() === "brackets"}><IconAgentOpenCode {...props} /></Match>
      <Match when={iconKey() === "bot"}><IconBot {...props} /></Match>
      <Match when={iconKey() === "bolt"}><IconBolt {...props} /></Match>
      <Match when={iconKey() === "compass"}><IconCompass {...props} /></Match>
      <Match when={iconKey() === "cube"}><IconCube {...props} /></Match>
      <Match when={iconKey() === "dna"}><IconDna {...props} /></Match>
      <Match when={iconKey() === "radar"}><IconRadar {...props} /></Match>
      <Match when={iconKey() === "shield"}><IconShield {...props} /></Match>
      <Match when={iconKey() === "cpu"}><IconCpu {...props} /></Match>
      <Match when={iconKey() === "flame"}><IconFlame {...props} /></Match>
      <Match when={iconKey() === "layers"}><IconLayers {...props} /></Match>
      <Match when={iconKey() === "orbit"}><IconOrbit {...props} /></Match>
      <Match when={iconKey() === "network"}><IconNetwork {...props} /></Match>
      <Match when={iconKey() === "sparkles"}><IconSparkles {...props} /></Match>
      <Match when={iconKey() === "wand"}><IconWand {...props} /></Match>
      <Match when={iconKey() === "feather"}><IconFeather {...props} /></Match>
      <Match when={iconKey() === "aperture"}><IconAperture {...props} /></Match>
      <Match when={iconKey() === "terminal"}><IconTerminal {...props} /></Match>
      <Match when={iconKey() === "pulse"}><IconPulse {...props} /></Match>
      <Match when={iconKey() === "globe"}><IconGlobe {...props} /></Match>
      <Match when={iconKey() === "zap"}><IconZap {...props} /></Match>
    </Switch>
  );
}
