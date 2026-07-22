// All UI icons live here. Each takes a `size` and draws with `currentColor`, so
// it inherits text color and the active accent from wherever it's placed.

interface IconProps {
  size?: number;
}

/** The moin brand mark: two uprights over a pair of nested download chevrons,
 *  reading as an "M". Stroke-based so it follows the accent via currentColor. */
export function Logo({ size = 28 }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <path
        fill="currentColor"
        d="M6.25 17L6.25 5.438L12 10.04L17.75 5.438L17.75 17L16.25 15.8L16.25 8.56L12 11.963L7.75 8.56L7.75 15.8Z"
      />
      <path
        fill="currentColor"
        d="M9 11.48L12 13.88L15 11.48L15 13.4L12 15.8L9 13.4Z"
      />
    </svg>
  );
}

/** The complete moin app icon: dark rounded tile plus the mark. Fixed brand
 *  colors (this is the app badge, so it doesn't follow the accent). */
export function BrandMark({ size = 36 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <defs>
        <linearGradient
          id="moin-tile"
          x1="0"
          y1="0"
          x2="0"
          y2="24"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#161c2b" />
          <stop offset="1" stopColor="#0c1017" />
        </linearGradient>
      </defs>
      <rect width="24" height="24" rx="5.4" fill="url(#moin-tile)" />
      <path
        fill="#6f8bff"
        d="M6.25 17L6.25 5.438L12 10.04L17.75 5.438L17.75 17L16.25 15.8L16.25 8.56L12 11.963L7.75 8.56L7.75 15.8Z"
      />
      <path
        fill="#6f8bff"
        d="M9 11.48L12 13.88L15 11.48L15 13.4L12 15.8L9 13.4Z"
      />
    </svg>
  );
}

/** A small filled caret. Points down by default; rotate 180° for ascending. */
export function SortArrowIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path fill="currentColor" d="m12 15l-5-5h10z" />
    </svg>
  );
}

export function AddIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path fill="currentColor" d="M11 13H5v-2h6V5h2v6h6v2h-6v6h-2z" />
    </svg>
  );
}

export function DownloadsIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="m12 16l-5-5l1.4-1.45l2.6 2.6V4h2v8.15l2.6-2.6L17 11zm-6 4q-.825 0-1.412-.587T4 18v-3h2v3h12v-3h2v3q0 .825-.587 1.413T18 20z"
      />
    </svg>
  );
}

export function CompletedIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="m9.55 16l-5.675-5.675L5.3 8.9l4.25 4.25L18.7 4l1.425 1.425zM5 20v-2h14v2z"
      />
    </svg>
  );
}

/** The Categories rail mark: a triangle, circle, and square — three shapes for
 *  three buckets. */
export function CategoriesIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M6.5 11L12 2l5.5 9zm11 11q-1.875 0-3.187-1.312T13 17.5t1.313-3.187T17.5 13t3.188 1.313T22 17.5t-1.312 3.188T17.5 22M3 21.5v-8h8v8zM17.5 20q1.05 0 1.775-.725T20 17.5t-.725-1.775T17.5 15t-1.775.725T15 17.5t.725 1.775T17.5 20M5 19.5h4v-4H5zM10.05 9h3.9L12 5.85z"
      />
    </svg>
  );
}

/** A grab handle for drag-to-reorder rows: two stacked bars. */
export function DragHandleIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path fill="currentColor" d="M4 15v-2h16v2zm0-4V9h16v2z" />
    </svg>
  );
}

export function SettingsIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M12 8a4 4 0 0 1 4 4a4 4 0 0 1-4 4a4 4 0 0 1-4-4a4 4 0 0 1 4-4m0 2a2 2 0 0 0-2 2a2 2 0 0 0 2 2a2 2 0 0 0 2-2a2 2 0 0 0-2-2m-2 12c-.25 0-.46-.18-.5-.42l-.37-2.65c-.63-.25-1.17-.59-1.69-.99l-2.49 1.01c-.22.08-.49 0-.61-.22l-2-3.46a.493.493 0 0 1 .12-.64l2.11-1.66L4.5 12l.07-1l-2.11-1.63a.493.493 0 0 1-.12-.64l2-3.46c.12-.22.39-.31.61-.22l2.49 1c.52-.39 1.06-.73 1.69-.98l.37-2.65c.04-.24.25-.42.5-.42h4c.25 0 .46.18.5.42l.37 2.65c.63.25 1.17.59 1.69.98l2.49-1c.22-.09.49 0 .61.22l2 3.46c.13.22.07.49-.12.64L19.43 11l.07 1l-.07 1l2.11 1.63c.19.15.25.42.12.64l-2 3.46c-.12.22-.39.31-.61.22l-2.49-1c-.52.39-1.06.73-1.69.98l-.37 2.65c-.04.24-.25.42-.5.42zm1.25-18l-.37 2.61c-1.2.25-2.26.89-3.03 1.78L5.44 7.35l-.75 1.3L6.8 10.2a5.55 5.55 0 0 0 0 3.6l-2.12 1.56l.75 1.3l2.43-1.04c.77.88 1.82 1.52 3.01 1.76l.37 2.62h1.52l.37-2.61c1.19-.25 2.24-.89 3.01-1.77l2.43 1.04l.75-1.3l-2.12-1.55c.4-1.17.4-2.44 0-3.61l2.11-1.55l-.75-1.3l-2.41 1.04a5.42 5.42 0 0 0-3.03-1.77L12.75 4z"
      />
    </svg>
  );
}

/** Animated sun ⇄ moon toggle (toggles.dev "spin"). Reflects the current theme
 *  via the root `[data-theme]` attribute and morphs when it changes — see the
 *  `.sun-moon` rules in moin.css. A clip-path bites a crescent as the disc grows;
 *  the rays spin 45° and shrink away. */
export function ThemeToggleIcon({ size = 20 }: IconProps) {
  return (
    <svg
      className="sun-moon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      <defs>
        <clipPath id="sun-moon-clip">
          <path className="sm-clip" d="M0 0h25a1 1 0 0010 10v14H0Z" />
        </clipPath>
      </defs>
      <g stroke="currentColor" strokeLinecap="round" strokeLinejoin="round">
        <circle
          className="sm-core"
          cx="12"
          cy="12"
          r="5"
          fill="currentColor"
          clipPath="url(#sun-moon-clip)"
        />
        <g className="sm-beams" fill="none" strokeWidth="2">
          <path d="M12 1.4v2.4" />
          <path d="m20.3 3.7-2.5 2.5" />
          <path d="M22.6 12h-2.4" />
          <path d="M12 22.6v-2.4" />
          <path d="M1.4 12h2.4" />
          <path d="m20.3 20.3-2.5-2.5" />
          <path d="m3.7 20.3 2.5-2.5" />
          <path d="m3.7 3.7 2.5 2.5" />
        </g>
      </g>
    </svg>
  );
}

export function MoonIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M12 21q-3.775 0-6.387-2.613T3 12q0-3.45 2.25-5.988T11 3.05q.325-.05.575.088t.4.362t.163.525t-.188.575q-.425.65-.638 1.375T11.1 7.5q0 2.25 1.575 3.825T16.5 12.9q.775 0 1.538-.225t1.362-.625q.275-.175.563-.162t.512.137q.25.125.388.375t.087.6q-.35 3.45-2.937 5.725T12 21"
      />
    </svg>
  );
}

export function SunIcon({ size = 24 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M8.463 15.538Q7 14.075 7 12t1.463-3.537T12 7t3.538 1.463T17 12t-1.463 3.538T12 17t-3.537-1.463M2 13q-.425 0-.712-.288T1 12t.288-.712T2 11h2q.425 0 .713.288T5 12t-.288.713T4 13zm18 0q-.425 0-.712-.288T19 12t.288-.712T20 11h2q.425 0 .713.288T23 12t-.288.713T22 13zm-8.712-8.287Q11 4.425 11 4V2q0-.425.288-.712T12 1t.713.288T13 2v2q0 .425-.288.713T12 5t-.712-.288m0 18Q11 22.426 11 22v-2q0-.425.288-.712T12 19t.713.288T13 20v2q0 .425-.288.713T12 23t-.712-.288M5.65 7.05L4.575 6q-.3-.275-.288-.7t.288-.725q.3-.3.725-.3t.7.3L7.05 5.65q.275.3.275.7t-.275.7t-.687.288t-.713-.288M18 19.425l-1.05-1.075q-.275-.3-.275-.712t.275-.688q.275-.3.688-.287t.712.287L19.425 18q.3.275.288.7t-.288.725q-.3.3-.725.3t-.7-.3M16.95 7.05q-.3-.275-.288-.687t.288-.713L18 4.575q.275-.3.7-.288t.725.288q.3.3.3.725t-.3.7L18.35 7.05q-.3.275-.7.275t-.7-.275M4.575 19.425q-.3-.3-.3-.725t.3-.7l1.075-1.05q.3-.275.712-.275t.688.275q.3.275.288.688t-.288.712L6 19.425q-.275.3-.7.288t-.725-.288"
      />
    </svg>
  );
}
