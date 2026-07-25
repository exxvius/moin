import { useSyncExternalStore } from "react";

// Performance mode: a single switch that strips the UI down to flat paint —
// no animations, no glass blur, no glows, no cursor effects. Aimed at people
// running thousands of tasks at once, where every per-row effect is multiplied
// by whatever is on screen and the compositor becomes the bottleneck.
//
// It's a display preference, so it lives in localStorage next to the theme and
// accent rather than in the backend settings file, and it's read synchronously
// at startup (no async round-trip, no first-frame flash of the full look).
//
// State lives here, module-level, because two places need to agree on it: the
// stylesheet hook (the `data-perf` root attribute, see styles/performance.css)
// and the JS effects that shouldn't run at all when it's on.

const KEY = "moin-perf-mode";
/** Root attribute every performance-mode CSS rule keys off. */
const ATTR = "data-perf";

let enabled = localStorage.getItem(KEY) === "1";
const listeners = new Set<() => void>();

function apply(): void {
  document.documentElement.toggleAttribute(ATTR, enabled);
}

/** Put the saved preference on the document before React's first paint. */
export function initPerfMode(): void {
  apply();
}

export function setPerfMode(on: boolean): void {
  if (on === enabled) return;
  enabled = on;
  localStorage.setItem(KEY, on ? "1" : "0");
  apply();
  for (const notify of listeners) notify();
}

function subscribe(notify: () => void): () => void {
  listeners.add(notify);
  return () => {
    listeners.delete(notify);
  };
}

export function usePerfMode(): boolean {
  return useSyncExternalStore(subscribe, () => enabled);
}
