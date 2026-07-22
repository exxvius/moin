import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

export type MenuEntry =
  | {
      label: string;
      onClick: () => void;
      danger?: boolean;
      disabled?: boolean;
      /** Keep the menu open after clicking (e.g. to reveal more options). */
      keepOpen?: boolean;
    }
  | { separator: true };

interface Props {
  x: number;
  y: number;
  items: MenuEntry[];
  /** Optional label shown at the top, e.g. how many rows the actions hit. */
  heading?: string;
  onClose: () => void;
}

/** A themed right-click menu, portaled to the body and clamped to the viewport.
 *  Closes on outside click, Esc, scroll, or resize. */
export function ContextMenu({ x, y, items, heading, onClose }: Props) {
  const ref = useRef<HTMLUListElement>(null);
  const [pos, setPos] = useState({ x, y });

  // Flip/clamp so the menu never spills off-screen.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const nx = Math.min(x, window.innerWidth - r.width - 8);
    const ny = Math.min(y, window.innerHeight - r.height - 8);
    setPos({ x: Math.max(8, nx), y: Math.max(8, ny) });
  }, [x, y, items.length]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onClose);
    window.addEventListener("scroll", onClose, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("scroll", onClose, true);
    };
  }, [onClose]);

  return createPortal(
    <ul
      ref={ref}
      className="ctx-menu"
      role="menu"
      style={{ position: "fixed", left: pos.x, top: pos.y }}
    >
      {heading && (
        <li className="ctx-heading" aria-hidden>
          {heading}
        </li>
      )}
      {items.map((it, i) =>
        "separator" in it ? (
          <li key={i} className="ctx-sep" aria-hidden />
        ) : (
          <li
            key={i}
            role="menuitem"
            aria-disabled={it.disabled}
            className={`ctx-item${it.danger ? " danger" : ""}${
              it.disabled ? " disabled" : ""
            }`}
            onClick={() => {
              if (it.disabled) return;
              it.onClick();
              if (!it.keepOpen) onClose();
            }}
          >
            {it.label}
          </li>
        ),
      )}
    </ul>,
    document.body,
  );
}
