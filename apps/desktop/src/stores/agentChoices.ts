import { createSignal } from "solid-js";
import type { AgentChoice } from "../bindings";
import { fetchAgentChoices } from "../ipc/agentChoices";
import { subscribeDaemon, type DaemonEvent } from "../ipc/rpc";

/// Detected agent runtimes only change when the operator's config changes, so every caller (the
/// Spawn dialog, reopened as often as a lane needs a new agent) shares one cached result instead
/// of re-running `agent.detect` behind the daemon's single connection on every open.
const [cached, setCached] = createSignal<AgentChoice[] | null>(null);
let inflight: Promise<AgentChoice[]> | null = null;
let invalidationArmed = false;

function onDaemonEvent(event: DaemonEvent) {
  if (event.method === "event.config.changed") setCached(null);
}

function armInvalidation() {
  if (invalidationArmed) return;
  invalidationArmed = true;
  void subscribeDaemon(onDaemonEvent);
}

/// The last detected choices, if any caller has fetched them this session.
export function cachedAgentChoices(): AgentChoice[] | null {
  return cached();
}

/// Resolves from the cache when present; otherwise fetches once and shares the in-flight request
/// with any other concurrent caller.
export function loadAgentChoices(): Promise<AgentChoice[]> {
  armInvalidation();
  const have = cached();
  if (have) return Promise.resolve(have);
  if (inflight) return inflight;
  const request = fetchAgentChoices()
    .then((detected) => {
      setCached(detected);
      return detected;
    })
    .finally(() => {
      inflight = null;
    });
  inflight = request;
  return request;
}

/// Forces a fresh `agent.detect`, for a retry after a failed load.
export function refreshAgentChoices(): Promise<AgentChoice[]> {
  inflight = null;
  setCached(null);
  return loadAgentChoices();
}

// Test-only: clears the module-level cache and invalidation subscription between test cases.
export function resetAgentChoicesCacheForTests(): void {
  setCached(null);
  inflight = null;
  invalidationArmed = false;
}
