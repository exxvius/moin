import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

export interface GridItem {
  value: string;
  /** The cell's visual content (a color swatch or an icon glyph). */
  render: ReactNode;
  /** Tooltip, since the grid carries no visible text labels. */
  title?: string;
}

interface Props {
  value: string;
  items: GridItem[];
  onChange: (value: string) => void;
  /** What the trigger button shows (the current swatch/icon). */
  trigger: ReactNode;
  ariaLabel?: string;
  /** Minimum cell size in px for the auto-filled grid. */
  cell?: number;
  /** `currentColor` for the popup — tints icon glyphs (swatches set their own bg). */
  menuColor?: string;
}

/**
 * A dropdown that opens a scrollable grid of choices instead of a vertical list —
 * used for the category color and icon pickers. Shares the portaled, flip-up
 * positioning and dismissal of {@link Select}; the popup is a wrapping grid of
 * square cells, one selected, click to choose and close.
 */
export function GridPicker({
  value,
  items,
  onChange,
  trigger,
  ariaLabel,
  cell = 34,
  menuColor,
}: Props) {
  const [open, setOpen] = useState(false);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const [placement, setPlacement] = useState<"down" | "up">("down");
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (open && triggerRef.current)
      setRect(triggerRef.current.getBoundingClientRect());
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !rect || !menuRef.current) return;
    const margin = 8;
    const menuHeight = menuRef.current.offsetHeight;
    const spaceBelow = window.innerHeight - rect.bottom - margin;
    const spaceAbove = rect.top - margin;
    setPlacement(
      menuHeight > spaceBelow && spaceAbove > spaceBelow ? "up" : "down",
    );
  }, [open, rect, items.length]);

  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node;
      if (triggerRef.current?.contains(t) || menuRef.current?.contains(t))
        return;
      close();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    const onScroll = (e: Event) => {
      if (menuRef.current?.contains(e.target as Node)) return;
      close();
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
    };
  }, [open]);

  let menuStyle: CSSProperties = {};
  if (rect) {
    const width = Math.max(rect.width, 248);
    const left = Math.max(8, rect.right - width);
    const base: CSSProperties = {
      width,
      gridTemplateColumns: `repeat(auto-fill, minmax(${cell}px, 1fr))`,
      ...(menuColor ? { color: menuColor } : {}),
    };
    menuStyle =
      placement === "up"
        ? {
            position: "fixed",
            bottom: window.innerHeight - rect.top + 6,
            left,
            ...base,
          }
        : { position: "fixed", top: rect.bottom + 6, left, ...base };
  }

  return (
    <div className={`sel grid-sel${open ? " open" : ""}`}>
      <button
        ref={triggerRef}
        type="button"
        className="grid-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((o) => !o)}
      >
        {trigger}
      </button>

      {open &&
        rect &&
        createPortal(
          <div ref={menuRef} className="grid-menu" role="listbox" style={menuStyle}>
            {items.map((it) => (
              <button
                key={it.value}
                type="button"
                role="option"
                aria-selected={it.value === value}
                title={it.title}
                aria-label={it.title}
                className={`grid-cell${it.value === value ? " selected" : ""}`}
                onClick={() => {
                  onChange(it.value);
                  setOpen(false);
                }}
              >
                {it.render}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}
