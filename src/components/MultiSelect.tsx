import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { SortArrowIcon } from "./icons";

export interface MultiSelectOption {
  value: string;
  label: ReactNode;
}

interface Props {
  /** Currently-on values. Membership drives the highlight on each option. */
  selected: Set<string>;
  options: MultiSelectOption[];
  /** Toggle one value on/off. The menu stays open so several can be picked. */
  onToggle: (value: string) => void;
  /** What the trigger shows (a summary the caller builds from `selected`). */
  trigger: ReactNode;
  ariaLabel?: string;
  caret?: boolean;
}

/**
 * A themed multi-select dropdown: clicking an option toggles it and keeps the
 * menu open, so a filter can hold several values at once. Selected options are
 * shown by the same highlight the single Select uses — no checkboxes. Shares the
 * `.sel-*` styling and the portaled, flip-up positioning of {@link Select}.
 */
export function MultiSelect({
  selected,
  options,
  onToggle,
  trigger,
  ariaLabel,
  caret,
}: Props) {
  const [open, setOpen] = useState(false);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const [placement, setPlacement] = useState<"down" | "up">("down");
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLUListElement>(null);

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
  }, [open, rect, options.length]);

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
    const width = Math.max(rect.width, 210);
    const left = Math.max(8, rect.right - width);
    menuStyle =
      placement === "up"
        ? {
            position: "fixed",
            bottom: window.innerHeight - rect.top + 6,
            left,
            width,
          }
        : { position: "fixed", top: rect.bottom + 6, left, width };
  }

  return (
    <div className={`sel${open ? " open" : ""}`}>
      <button
        ref={triggerRef}
        type="button"
        className="sel-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="sel-value">{trigger}</span>
        {caret && (
          <span className="sel-caret" aria-hidden>
            <SortArrowIcon size={13} />
          </span>
        )}
      </button>

      {open &&
        rect &&
        createPortal(
          <ul ref={menuRef} className="sel-menu" role="listbox" style={menuStyle}>
            {options.map((o) => {
              const on = selected.has(o.value);
              return (
                <li
                  key={o.value}
                  role="option"
                  aria-selected={on}
                  className={`sel-option${on ? " selected" : ""}`}
                  onClick={() => onToggle(o.value)}
                >
                  {o.label}
                </li>
              );
            })}
          </ul>,
          document.body,
        )}
    </div>
  );
}
