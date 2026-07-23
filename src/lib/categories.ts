// Shared category display helpers, reused by the Add view, the Downloads list,
// and the Categories manager.

import { ACCENTS } from "./accent";
import { formatBytes } from "./format";
import type { AddMethodKind, Category, Trigger } from "./types";

/** Colors a category can wear: the app accents plus a category-only "Black".
 *  Black isn't an app theme accent (it has no `[data-accent]` rule) — it's just a
 *  swatch, which is all a category color needs, since cards tint straight from the
 *  raw value via `--cat`. */
export const CATEGORY_COLORS: { id: string; label: string; swatch: string }[] = [
  ...ACCENTS,
  // Black/White are theme-reversed neutrals (see --cat-black/--cat-white in
  // tokens.css) so each stays visible against the current background.
  { id: "black", label: "Black", swatch: "var(--cat-black)" },
  { id: "white", label: "White", swatch: "var(--cat-white)" },
];

/** Swatch color for a category's color id, or a neutral fallback. `"accent"` is a
 *  special id that follows the app's current theme accent. */
export function categorySwatch(color: string): string {
  if (color === "accent") return "var(--accent)";
  return CATEGORY_COLORS.find((a) => a.id === color)?.swatch ?? "var(--text-dim)";
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
  "browser-capture": "Browser",
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
