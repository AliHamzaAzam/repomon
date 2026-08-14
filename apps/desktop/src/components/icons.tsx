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

export function IconBrandClaude(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      fill-rule="nonzero"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" />
    </svg>
  );
}

export function IconBrandAntigravity(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      fill-rule="evenodd"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z" />
    </svg>
  );
}

export function IconBrandOpenAI(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      fill-rule="evenodd"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M9.205 8.658v-2.26c0-.19.072-.333.238-.428l4.543-2.616c.619-.357 1.356-.523 2.117-.523 2.854 0 4.662 2.212 4.662 4.566 0 .167 0 .357-.024.547l-4.71-2.759a.797.797 0 00-.856 0l-5.97 3.473zm10.609 8.8V12.06c0-.333-.143-.57-.429-.737l-5.97-3.473 1.95-1.118a.433.433 0 01.476 0l4.543 2.617c1.309.76 2.189 2.378 2.189 3.948 0 1.808-1.07 3.473-2.76 4.163zM7.802 12.703l-1.95-1.142c-.167-.095-.239-.238-.239-.428V5.899c0-2.545 1.95-4.472 4.591-4.472 1 0 1.927.333 2.712.928L8.23 5.067c-.285.166-.428.404-.428.737v6.898zM12 15.128l-2.795-1.57v-3.33L12 8.658l2.795 1.57v3.33L12 15.128zm1.796 7.23c-1 0-1.927-.332-2.712-.927l4.686-2.712c.285-.166.428-.404.428-.737v-6.898l1.974 1.142c.167.095.238.238.238.428v5.233c0 2.545-1.974 4.472-4.614 4.472zm-5.637-5.303l-4.544-2.617c-1.308-.761-2.188-2.378-2.188-3.948A4.482 4.482 0 014.21 6.327v5.423c0 .333.143.571.428.738l5.947 3.449-1.95 1.118a.432.432 0 01-.476 0zm-.262 3.9c-2.688 0-4.662-2.021-4.662-4.519 0-.19.024-.38.047-.57l4.686 2.71c.286.167.571.167.856 0l5.97-3.448v2.26c0 .19-.07.333-.237.428l-4.543 2.616c-.619.357-1.356.523-2.117.523zm5.899 2.83a5.947 5.947 0 005.827-4.756C22.287 18.339 24 15.84 24 13.296c0-1.665-.713-3.282-1.998-4.448.119-.5.19-.999.19-1.498 0-3.401-2.759-5.947-5.946-5.947-.642 0-1.26.095-1.88.31A5.962 5.962 0 0010.205 0a5.947 5.947 0 00-5.827 4.757C1.713 5.447 0 7.945 0 10.49c0 1.666.713 3.283 1.998 4.448-.119.5-.19 1-.19 1.499 0 3.401 2.759 5.946 5.946 5.946.642 0 1.26-.095 1.88-.309a5.96 5.96 0 004.162 1.713z" />
    </svg>
  );
}

export function IconBrandOpenCode(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      fill-rule="evenodd"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M16 6H8v12h8V6zm4 16H4V2h16v20z" />
    </svg>
  );
}

export function IconBrandCursor(props: IconProps): JSX.Element {
  const s = () => props.size ?? 14;
  return (
    <svg
      width={s()}
      height={s()}
      viewBox="0 0 24 24"
      fill="currentColor"
      fill-rule="evenodd"
      class={props.class}
      aria-hidden="true"
    >
      <path d="M22.106 5.68L12.5.135a.998.998 0 00-.998 0L1.893 5.68a.84.84 0 00-.419.726v11.186c0 .3.16.577.42.727l9.607 5.547a.999.999 0 00.998 0l9.608-5.547a.84.84 0 00.42-.727V6.407a.84.84 0 00-.42-.726zm-.603 1.176L12.228 22.92c-.063.108-.228.064-.228-.061V12.34a.59.59 0 00-.295-.51l-9.11-5.26c-.107-.062-.063-.228.062-.228h18.55c.264 0 .428.286.296.514z" />
    </svg>
  );
}

export interface AgentIconCatalogEntry {
  id: string;
  label: string;
  category: "brands" | "agents" | "system" | "creative" | "analysis";
  Icon: (props: IconProps) => JSX.Element;
}

export const AGENT_ICON_CATALOG: AgentIconCatalogEntry[] = [
  // Official Brand Marks
  { id: "brand-claude", label: "Claude Brand Mark", category: "brands", Icon: IconBrandClaude },
  { id: "brand-antigravity", label: "Antigravity Brand Mark", category: "brands", Icon: IconBrandAntigravity },
  { id: "brand-openai", label: "OpenAI / Codex Mark", category: "brands", Icon: IconBrandOpenAI },
  { id: "brand-opencode", label: "OpenCode Brand Mark", category: "brands", Icon: IconBrandOpenCode },
  { id: "brand-cursor", label: "Cursor Brand Mark", category: "brands", Icon: IconBrandCursor },

  // Abstract Geometric Library
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

  // Default brand icons for the five requested kinds:
  if (raw === "claude-code" || raw === "claude") return "brand-claude";
  if (raw === "antigravity" || raw === "agy") return "brand-antigravity";
  if (raw === "codex") return "brand-openai";
  if (raw === "opencode") return "brand-opencode";
  if (raw === "cursor") return "brand-cursor";

  // Abstract geometric defaults for other agents:
  if (raw === "aider") return "binary-orbit";

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
      {/* Official Brand Marks */}
      <Match when={iconKey() === "brand-claude"}><IconBrandClaude {...props} /></Match>
      <Match when={iconKey() === "brand-antigravity"}><IconBrandAntigravity {...props} /></Match>
      <Match when={iconKey() === "brand-openai"}><IconBrandOpenAI {...props} /></Match>
      <Match when={iconKey() === "brand-opencode"}><IconBrandOpenCode {...props} /></Match>
      <Match when={iconKey() === "brand-cursor"}><IconBrandCursor {...props} /></Match>

      {/* Abstract Geometric Library */}
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
