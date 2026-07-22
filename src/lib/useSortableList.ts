// Pointer-driven drag-to-reorder for a vertical list, with the grabbed item
// tracking the cursor and its neighbors sliding out of the way (FLIP), then the
// item settling into place on drop.
//
// Why not HTML5 DnD: Tauri's OS drag-drop handler swallows in-webview drag
// events on Windows. Why pointer + offsetTop: `offsetTop` is the settled layout
// position, immune to the in-flight FLIP transforms that otherwise cause a
// measurement feedback loop. Window listeners guarantee the drop fires even
// though the grabbed row moves in the DOM mid-drag.
//
// The hook is purely imperative (refs + inline styles), so it needs no state of
// its own — the caller's `onReorder` drives the re-render that re-runs the FLIP.
// Inline transform/transition/z-index/box-shadow are never part of the caller's
// React style object, so React won't clear them between renders.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  type PointerEvent as ReactPointerEvent,
} from "react";

interface Options {
  /** Move an item from one index to another (updates the caller's list). */
  onReorder: (from: number, to: number) => void;
  /** Fired once when the drag ends (e.g. to persist the new order). */
  onDrop?: () => void;
}

interface DragState {
  applied: number;
  startY: number;
  startTop: number;
  pointerY: number;
  el: HTMLElement;
}

const NEIGHBOR_EASE = "transform 200ms cubic-bezier(0.2, 0.9, 0.3, 1)";
const SETTLE_EASE = "transform 180ms cubic-bezier(0.2, 0.9, 0.3, 1)";

export function useSortableList<C extends HTMLElement = HTMLDivElement>({
  onReorder,
  onDrop,
}: Options) {
  const containerRef = useRef<C | null>(null);
  const drag = useRef<DragState | null>(null);
  const prevTops = useRef<WeakMap<Element, number>>(new WeakMap());

  // Glue the grabbed element to the cursor: its visual top stays at
  // (start slot top + pointer delta) no matter which slot it currently occupies.
  const followDragged = () => {
    const st = drag.current;
    if (!st) return;
    const dy = st.pointerY - st.startY - (st.el.offsetTop - st.startTop);
    st.el.style.transition = "none";
    st.el.style.transform = `translateY(${dy}px)`;
  };

  const handlers = useRef<{ move: (e: PointerEvent) => void; up: () => void }>();
  if (!handlers.current) {
    const move = (e: PointerEvent) => {
      const st = drag.current;
      const container = containerRef.current;
      if (!st || !container) return;
      st.pointerY = e.clientY;
      followDragged();

      const rows = Array.from(container.children) as HTMLElement[];
      const y = e.clientY - container.getBoundingClientRect().top;
      let target = 0;
      for (const r of rows) {
        if (y > r.offsetTop + r.offsetHeight / 2) target++;
      }
      target = Math.max(0, Math.min(rows.length - 1, target));
      if (target !== st.applied) {
        const from = st.applied;
        st.applied = target;
        onReorder(from, target);
      }
    };

    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      const st = drag.current;
      drag.current = null;
      if (st) {
        // Settle: the array order already matches the visual order, so easing the
        // transform back to zero slides the item into its final slot.
        const el = st.el;
        el.style.transition = SETTLE_EASE;
        el.style.transform = "";
        const cleanup = () => {
          el.style.transition = "";
          el.style.transform = "";
          el.removeAttribute("data-dragging");
          el.removeEventListener("transitionend", cleanup);
        };
        el.addEventListener("transitionend", cleanup);
        window.setTimeout(cleanup, 260);
      }
      onDrop?.();
    };

    handlers.current = { move, up };
  }

  const startDrag = (index: number, e: ReactPointerEvent) => {
    e.preventDefault();
    const container = containerRef.current;
    if (!container) return;
    const el = container.children[index] as HTMLElement | undefined;
    if (!el) return;
    drag.current = {
      applied: index,
      startY: e.clientY,
      startTop: el.offsetTop,
      pointerY: e.clientY,
      el,
    };
    // The lifted look (z-index, frost, shadow) lives in CSS keyed on this
    // attribute — React never manages it, so re-renders during the drag leave it
    // alone. We only drive the transform here.
    el.setAttribute("data-dragging", "");
    followDragged();
    const { move, up } = handlers.current!;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  };

  // After every render: FLIP the neighbors from their old positions to their new
  // ones, and re-glue the grabbed element (its slot may have changed).
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const rows = Array.from(container.children) as HTMLElement[];
    const st = drag.current;
    for (const el of rows) {
      const top = el.offsetTop;
      const prev = prevTops.current.get(el);
      const isDragged = st != null && el === st.el;
      if (!isDragged && prev != null && prev !== top) {
        el.style.transition = "none";
        el.style.transform = `translateY(${prev - top}px)`;
        requestAnimationFrame(() => {
          el.style.transition = NEIGHBOR_EASE;
          el.style.transform = "";
        });
      }
      prevTops.current.set(el, top);
    }
    if (st) followDragged();
  });

  useEffect(() => {
    const h = handlers.current!;
    return () => {
      window.removeEventListener("pointermove", h.move);
      window.removeEventListener("pointerup", h.up);
      window.removeEventListener("pointercancel", h.up);
    };
  }, []);

  return { containerRef, startDrag };
}
