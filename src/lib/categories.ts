// Shared category display helpers, reused by the Add view, the Downloads list,
// and the Categories manager.

import { ACCENTS } from "./accent";
import { formatBytes } from "./format";
import type { AddMethodKind, Category, Trigger } from "./types";

/** Swatch color for a category's accent id, or a neutral fallback. */
export function categorySwatch(color: string): string {
  return ACCENTS.find((a) => a.id === color)?.swatch ?? "var(--text-dim)";
}

/** The category with this id from a list, or undefined when null/missing. */
export function findCategory(
  cats: Category[],
  id: string | null,
): Category | undefined {
  return id ? cats.find((c) => c.id === id) : undefined;
}

export const ADD_METHOD_LABEL: Record<AddMethodKind, string> = {
  "manual-link": "Manual link",
  "manual-torrent": "Manual torrent",
  "watch-folder": "Watched folder",
  "watch-url-file": "Watched URL list",
};

/** A short human description of a single trigger. */
export function triggerLabel(t: Trigger): string {
  switch (t.type) {
    case "extension":
      return `Type: ${t.exts.map((e) => `.${e.replace(/^\./, "")}`).join(", ")}`;
    case "size": {
      if (t.min != null && t.max != null)
        return `Size ${formatBytes(t.min)}–${formatBytes(t.max)}`;
      if (t.min != null) return `Size ≥ ${formatBytes(t.min)}`;
      if (t.max != null) return `Size ≤ ${formatBytes(t.max)}`;
      return "Any size";
    }
    case "url-pattern":
      return `URL: ${t.patterns.join(", ")}`;
    case "name-pattern":
      return `Name: ${t.patterns.join(", ")}`;
  }
}

/** One-line summary of a category's sources + triggers for list rows. */
export function ruleSummary(cat: Category): string {
  const parts: string[] = [];
  if (cat.sources.length > 0) {
    parts.push(`From ${cat.sources.map((s) => ADD_METHOD_LABEL[s]).join(", ")}`);
  }
  parts.push(...cat.triggers.map(triggerLabel));
  if (parts.length === 0) return "No rules yet — won't auto-match";
  return parts.join(" · ");
}
