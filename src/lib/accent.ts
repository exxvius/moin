import { useEffect, useState } from "react";

/** Accent presets (must match the `[data-accent="…"]` rules in tokens.css). */
export type Accent =
  | "emerald"
  | "green"
  | "lime"
  | "teal"
  | "cyan"
  | "sky"
  | "blue"
  | "indigo"
  | "violet"
  | "purple"
  | "fuchsia"
  | "magenta"
  | "pink"
  | "rose"
  | "red"
  | "orange"
  | "amber";

export interface AccentOption {
  id: Accent;
  label: string;
  /** A representative swatch color (dark-mode approximation) for the picker. */
  swatch: string;
}

// Ordered around the hue wheel for a coherent palette in the picker.
export const ACCENTS: AccentOption[] = [
  { id: "emerald", label: "Emerald", swatch: "oklch(80% 0.15 165)" },
  { id: "green", label: "Green", swatch: "oklch(80% 0.16 150)" },
  { id: "lime", label: "Lime", swatch: "oklch(85% 0.17 128)" },
  { id: "teal", label: "Teal", swatch: "oklch(80% 0.13 195)" },
  { id: "cyan", label: "Cyan", swatch: "oklch(80% 0.13 215)" },
  { id: "sky", label: "Sky", swatch: "oklch(80% 0.13 235)" },
  { id: "blue", label: "Blue", swatch: "oklch(76% 0.15 258)" },
  { id: "indigo", label: "Indigo", swatch: "oklch(72% 0.16 278)" },
  { id: "violet", label: "Violet", swatch: "oklch(74% 0.16 296)" },
  { id: "purple", label: "Purple", swatch: "oklch(72% 0.17 315)" },
  { id: "fuchsia", label: "Fuchsia", swatch: "oklch(74% 0.19 328)" },
  { id: "magenta", label: "Magenta", swatch: "oklch(72% 0.2 342)" },
  { id: "pink", label: "Pink", swatch: "oklch(78% 0.16 355)" },
  { id: "rose", label: "Rose", swatch: "oklch(74% 0.18 12)" },
  { id: "red", label: "Red", swatch: "oklch(70% 0.2 28)" },
  { id: "orange", label: "Orange", swatch: "oklch(78% 0.16 52)" },
  { id: "amber", label: "Amber", swatch: "oklch(83% 0.15 78)" },
];

const KEY = "moin-accent";
// Blue by default — moin's own identity.
const DEFAULT: Accent = "blue";

function isAccent(v: string | null): v is Accent {
  return ACCENTS.some((a) => a.id === v);
}

export function useAccent(): [Accent, (a: Accent) => void] {
  const [accent, setAccent] = useState<Accent>(() => {
    const stored = localStorage.getItem(KEY);
    return isAccent(stored) ? stored : DEFAULT;
  });

  useEffect(() => {
    document.documentElement.setAttribute("data-accent", accent);
    localStorage.setItem(KEY, accent);
  }, [accent]);

  return [accent, setAccent];
}
