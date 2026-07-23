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

export interface SelectOption {
  value: string;
  label: ReactNode;
}

interface Props {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel?: string;
  disabled?: boolean;
  /** Show a caret on the trigger (rotates when open). */
  caret?: boolean;
}

/**
 * A themed dropdown replacing the OS-rendered native `<select>`. The menu is
 * portaled to `document.body` with fixed positioning so glass cards (each their
 * own stacking context) can't paint over it.
 */
export function Select({
  value,
  options,
  onChange,
  ariaLabel,
  disabled,
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

  // Once the menu is measured, open it upward instead of down when it would
  // otherwise run off the bottom of the window and there's more room above.
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
    // Scrolling the page dismisses the menu so it never floats detached from its
    // trigger — but scrolling inside the menu must not.
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

  const current = options.find((o) => o.value === value);

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
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="sel-value">{current?.label ?? value}</span>
        {caret && (
          <span className="sel-caret" aria-hidden>
            <SortArrowIcon size={13} />
          </span>
        )}
      </button>

      {open &&
        rect &&
        createPortal(
          <ul
            ref={menuRef}
            className="sel-menu"
            role="listbox"
            style={menuStyle}
          >
            {options.map((o) => (
              <li
                key={o.value}
                role="option"
                aria-selected={o.value === value}
                className={`sel-option${o.value === value ? " selected" : ""}`}
                onClick={() => {
                  onChange(o.value);
                  setOpen(false);
                }}
              >
                {o.label}
              </li>
            ))}
          </ul>,
          document.body,
        )}
    </div>
  );
}
