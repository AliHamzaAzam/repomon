import { For, Show } from "solid-js";

import type { AgentSession } from "../bindings";
import { agentStateReason, controllerAgentState, stateIndicator } from "../stores/fleet";
import { agentLabel } from "./agentLabel";
import { AgentIcon } from "./icons";
import { Section, SectionButton, SectionNote } from "./RepomindSection";

/// Who is running in the home lane, in what state, and how to get to them.
///
/// The panel is the control room, not the conversation: a controller's own words are in its pane
/// in the terminal bay, so every row here ends in the one control that takes you there.
export interface RepomindControllersProps {
  sessions: AgentSession[];
  /// Puts this controller's pane in front in the terminal bay.
  onFocus: (session: AgentSession) => void;
}

export default function RepomindControllers(props: RepomindControllersProps) {
  const reasoned = () => props.sessions.filter((session) => agentStateReason(session));

  return (
    <Section title="Controllers" detail={String(props.sessions.length)}>
      <Show
        when={props.sessions.length}
        fallback={
          <SectionNote>
            No controller is running. Start one from the header, and it comes up already knowing
            your house rules, the goals in flight, and the state of every lane.
          </SectionNote>
        }
      >
        <ul class="space-y-0.5">
          <For each={props.sessions}>
            {(session) => {
              const state = () => stateIndicator(controllerAgentState(session));
              return (
                <li
                  class="flex items-center gap-1.5 rounded-md px-1.5 py-1 transition-colors hover:bg-raised/50"
                  title={agentStateReason(session) ?? undefined}
                >
                  <AgentIcon agent={session.agent} size={11} class="shrink-0 text-muted/70" />
                  <span class="min-w-0 flex-1 truncate text-xs text-foreground">
                    {agentLabel(session)}
                  </span>
                  <span class={`lane-status is-${state().tone}`}>{state().label}</span>
                  <SectionButton
                    label="Focus pane"
                    disabled={!session.tmux_window}
                    title={
                      session.tmux_window
                        ? `Bring ${agentLabel(session)} to the front of the terminal bay`
                        : "This session has no managed pane to focus"
                    }
                    onClick={() => props.onFocus(session)}
                  />
                </li>
              );
            }}
          </For>
        </ul>
        <Show when={reasoned().length}>
          <ul class="mt-1.5 space-y-0.5">
            <For each={reasoned()}>
              {(session) => (
                <li class="px-1.5 text-[11px] leading-snug text-muted">
                  {agentLabel(session)}: {agentStateReason(session)}
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Show>
    </Section>
  );
}
