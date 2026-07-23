// Multi-select for the downloads list: plain/ctrl/shift clicks plus a drag
// marquee. Click handling lives at the container level (a native mousedown
// listener) so it can tell a click apart from a rubber-band drag by distance.

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

// How far the pointer must move before a press turns into a marquee drag
// instead of a click.
const DRAG_THRESHOLD = 5;

/** Whether two id sets hold exactly the same members. */
function sameSet(a: Set<string>, b: Set<string>): boolean {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}

export interface Marquee {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Options {
  /** The scrolling list element that holds the `.dl-card` rows. */
  containerRef: RefObject<HTMLElement | null>;
  /** Card ids in their current visual order — drives range (shift) selection. */
  orderedIds: string[];
  /** True once the list is mounted, so the listener attaches when rows exist. */
  enabled: boolean;
  /** Plain single click on a row (used to toggle the card's expansion). */
  onActivate?: (id: string) => void;
}

export interface ListSelection {
  selected: Set<string>;
  isSelected: (id: string) => boolean;
  /** The live rubber-band rectangle in viewport coords, or null when idle. */
  marquee: Marquee | null;
  clear: () => void;
  selectAll: () => void;
  /** Make `id` the whole selection unless it's already part of it. */
  ensure: (id: string) => void;
}

export function useListSelection({
  containerRef,
  orderedIds,
  enabled,
  onActivate,
}: Options): ListSelection {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [marquee, setMarquee] = useState<Marquee | null>(null);
  const anchor = useRef<string | null>(null);

  // Mirror reactive values into refs so the native listener always reads fresh
  // data without re-attaching on every render.
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const orderRef = useRef(orderedIds);
  orderRef.current = orderedIds;
  const onActivateRef = useRef(onActivate);
  onActivateRef.current = onActivate;

  // Drop ids that have left the list (removed, filtered out) so stale ids never
  // reach the store actions.
  useEffect(() => {
    setSelected((prev) => {
      if (prev.size === 0) return prev;
      const live = new Set(orderedIds);
      let changed = false;
      const next = new Set<string>();
      for (const id of prev) {
        if (live.has(id)) next.add(id);
        else changed = true;
      }
      return changed ? next : prev;
    });
  }, [orderedIds]);

  const handleClick = useCallback(
    (id: string | null, additive: boolean, range: boolean) => {
      if (id == null) {
        // Empty space: a plain click clears; modified clicks keep the set.
        if (!additive && !range) {
          setSelected(new Set());
          anchor.current = null;
        }
        return;
      }

      if (range && anchor.current) {
        const order = orderRef.current;
        const a = order.indexOf(anchor.current);
        const b = order.indexOf(id);
        if (a !== -1 && b !== -1) {
          const [lo, hi] = a < b ? [a, b] : [b, a];
          const span = order.slice(lo, hi + 1);
          setSelected((prev) => {
            const next = additive ? new Set(prev) : new Set<string>();
            for (const r of span) next.add(r);
            return next;
          });
          return;
        }
      }

      if (additive) {
        setSelected((prev) => {
          const next = new Set(prev);
          if (next.has(id)) next.delete(id);
          else next.add(id);
          return next;
        });
        anchor.current = id;
        return;
      }

      // Plain click: this row becomes the sole selection and toggles open.
      setSelected(new Set([id]));
      anchor.current = id;
      onActivateRef.current?.(id);
    },
    [],
  );

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !enabled) return;

    const onMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      const target = e.target as HTMLElement;
      // Leave inner interactive content and selectable text alone.
      if (target.closest(".dl-detail, input, button, a, .selectable, .ss-bar"))
        return;

      const card = target.closest<HTMLElement>(".dl-card");
      const id = card?.dataset.id ?? null;
      const additive = e.ctrlKey || e.metaKey;
      const range = e.shiftKey;
      const startX = e.clientX;
      const startY = e.clientY;
      // A marquee started with a modifier adds to what's already selected.
      const base = additive ? new Set(selectedRef.current) : new Set<string>();
      let dragging = false;
      // Card boxes snapshotted once when the drag begins — they don't move during
      // a marquee, so we never re-read layout (a getBoundingClientRect per card per
      // mousemove was the lag). Only the visible rows are in the DOM (virtualized),
      // so this is cheap regardless of the total item count.
      let cardBoxes: { id: string; box: DOMRect }[] = [];
      let raf = 0;
      let latest: MouseEvent | null = null;

      const paint = () => {
        raf = 0;
        const ev = latest;
        if (!ev) return;
        const left = Math.min(startX, ev.clientX);
        const top = Math.min(startY, ev.clientY);
        const right = Math.max(startX, ev.clientX);
        const bottom = Math.max(startY, ev.clientY);

        // Clamp the drawn rectangle to the visible list so it never paints over
        // the header or spills past the scroll area.
        const cr = container.getBoundingClientRect();
        const vx = Math.max(cr.left, left);
        const vy = Math.max(cr.top, top);
        setMarquee({
          x: vx,
          y: vy,
          w: Math.max(0, Math.min(cr.right, right) - vx),
          h: Math.max(0, Math.min(cr.bottom, bottom) - vy),
        });

        const next = new Set(base);
        for (const { id: cid, box: b } of cardBoxes) {
          if (b.left < right && b.right > left && b.top < bottom && b.bottom > top)
            next.add(cid);
        }
        // Only push a new selection when it actually changed, so sweeping within
        // the same set of rows doesn't re-render the list every frame.
        setSelected((prev) => (sameSet(prev, next) ? prev : next));
      };

      const onMove = (ev: MouseEvent) => {
        if (!dragging && Math.hypot(ev.clientX - startX, ev.clientY - startY) < DRAG_THRESHOLD)
          return;
        if (!dragging) {
          dragging = true;
          document.body.style.userSelect = "none";
          cardBoxes = Array.from(
            container.querySelectorAll<HTMLElement>(".dl-card"),
          )
            .filter((el) => el.dataset.id)
            .map((el) => ({ id: el.dataset.id as string, box: el.getBoundingClientRect() }));
        }
        ev.preventDefault();
        latest = ev;
        if (!raf) raf = requestAnimationFrame(paint);
      };

      const onUp = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        document.body.style.userSelect = "";
        if (raf) cancelAnimationFrame(raf);
        if (dragging) {
          setMarquee(null);
          return;
        }
        handleClick(id, additive, range);
      };

      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    };

    container.addEventListener("mousedown", onMouseDown);
    return () => container.removeEventListener("mousedown", onMouseDown);
  }, [containerRef, enabled, handleClick]);

  // A left click anywhere outside the list clears the selection. Clicks inside
  // the list are already handled above; clicks on the context menu are spared,
  // since its actions operate on the current selection.
  useEffect(() => {
    if (!enabled) return;
    const onDocDown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      const target = e.target as HTMLElement;
      if (containerRef.current?.contains(target)) return;
      if (target.closest(".ctx-menu")) return;
      setSelected((prev) => (prev.size ? new Set() : prev));
      anchor.current = null;
    };
    document.addEventListener("mousedown", onDocDown);
    return () => document.removeEventListener("mousedown", onDocDown);
  }, [containerRef, enabled]);

  const isSelected = useCallback((id: string) => selected.has(id), [selected]);
  const clear = useCallback(() => {
    setSelected(new Set());
    anchor.current = null;
  }, []);
  const selectAll = useCallback(() => {
    setSelected(new Set(orderRef.current));
  }, []);
  const ensure = useCallback((id: string) => {
    setSelected((prev) => (prev.has(id) ? prev : new Set([id])));
    anchor.current = id;
  }, []);

  return { selected, isSelected, marquee, clear, selectAll, ensure };
}
