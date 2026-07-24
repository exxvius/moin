// A layer of "ghost" rectangles rendered behind the (clipped) downloads list.
// The list clips its rows on all sides, so a row's hover/selected glow can't
// spill past the viewport edges. Each ghost mirrors one visible row's box and
// carries only the glow, sitting outside the clip so the glow escapes the sides.
//
// Only rows that are BOTH highlighted (selected or hovered) AND on screen get a
// ghost, so a huge selection never spawns a huge pile of nodes.
//
// While anything is highlighted (or still fading out) a rAF loop re-places the
// ghosts every frame, so they stay glued to their rows through anything that
// moves a row — scrolling, a live-sort FLIP slide, expand/collapse. When a row
// stops being highlighted its ghost fades out (rather than snapping away) and is
// only then dropped.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from "react";

// Keep exit fades in step with the card's 240ms hover/select transitions.
const FADE_MS = 240;

interface Ghost {
  id: string;
  tone: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** True while the row is highlighted; false once it's fading out. */
  on: boolean;
  /** Timestamp (ms) at which a fading ghost may be dropped. */
  removeAt: number;
}

interface Props {
  /** The clipped viewport that holds the `.dl-card` rows. */
  viewportRef: RefObject<HTMLElement | null>;
  /** Ids the user has selected. */
  selectedIds: Set<string>;
  /** The glow tone (a CSS color, e.g. "var(--warn)") for a row, or null. */
  toneOf: (id: string) => string | null;
  /** Pause the rAF re-placement (e.g. during a marquee drag, when the rows don't
   *  move and the per-frame querySelectorAll + rect reads are pure overhead). */
  frozen?: boolean;
}

function sameGhosts(a: Ghost[], b: Ghost[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const p = a[i];
    const q = b[i];
    if (
      p.id !== q.id ||
      p.on !== q.on ||
      p.tone !== q.tone ||
      p.x !== q.x ||
      p.y !== q.y ||
      p.w !== q.w ||
      p.h !== q.h
    )
      return false;
  }
  return true;
}

export function GhostGlowLayer({
  viewportRef,
  selectedIds,
  toneOf,
  frozen = false,
}: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [ghosts, setGhosts] = useState<Ghost[]>([]);
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  // Latest props/state for the loop, without re-binding it every render.
  const selRef = useRef(selectedIds);
  selRef.current = selectedIds;
  const toneRef = useRef(toneOf);
  toneRef.current = toneOf;
  const hoverRef = useRef(hoveredId);
  hoverRef.current = hoveredId;
  const ghostsRef = useRef(ghosts);
  ghostsRef.current = ghosts;

  const tick = useCallback(() => {
    const root = rootRef.current;
    const viewport = viewportRef.current;
    if (!root || !viewport) return;

    const layer = root.getBoundingClientRect();
    const selected = selRef.current;
    const hoverId = hoverRef.current;
    const now = performance.now();

    // The currently highlighted, on-screen rows and their live boxes.
    const live = new Map<string, Omit<Ghost, "on" | "removeAt">>();
    for (const card of viewport.querySelectorAll<HTMLElement>(".dl-card")) {
      const id = card.dataset.id;
      if (!id || (!selected.has(id) && id !== hoverId)) continue;
      const r = card.getBoundingClientRect();
      if (r.bottom <= layer.top || r.top >= layer.bottom) continue; // off screen
      const tone = toneRef.current(id);
      if (!tone) continue;
      live.set(id, {
        id,
        tone,
        x: r.left - layer.left,
        y: r.top - layer.top,
        w: r.width,
        h: r.height,
      });
    }

    const prev = ghostsRef.current;
    const next: Ghost[] = [];
    for (const g of live.values()) next.push({ ...g, on: true, removeAt: 0 });
    // Carry over rows that just stopped being highlighted, fading in place,
    // until their fade window elapses.
    for (const g of prev) {
      if (live.has(g.id)) continue;
      const removeAt = g.on ? now + FADE_MS : g.removeAt;
      if (removeAt <= now) continue;
      next.push({ ...g, on: false, removeAt });
    }

    ghostsRef.current = next;
    setGhosts((cur) => (sameGhosts(cur, next) ? cur : next));
  }, [viewportRef]);

  // Track the hovered row so its glow escapes too.
  useEffect(() => {
    const vp = viewportRef.current;
    if (!vp) return;
    const onOver = (e: MouseEvent) =>
      setHoveredId(
        (e.target as HTMLElement).closest<HTMLElement>(".dl-card")?.dataset.id ??
          null,
      );
    const onLeave = () => setHoveredId(null);
    vp.addEventListener("mouseover", onOver);
    vp.addEventListener("mouseleave", onLeave);
    return () => {
      vp.removeEventListener("mouseover", onOver);
      vp.removeEventListener("mouseleave", onLeave);
    };
  }, [viewportRef]);

  // Run the loop while anything is highlighted or still fading out. It stops
  // itself once every ghost is gone.
  const active = selectedIds.size > 0 || hoveredId !== null;
  useEffect(() => {
    // While a drag owns the pointer the rows are static, so leave the current
    // ghosts in place and skip the per-frame querySelectorAll + rect reads.
    if (frozen) return;
    if (!active && ghostsRef.current.length === 0) return;
    let raf = 0;
    const loop = () => {
      tick();
      const keep =
        selRef.current.size > 0 ||
        hoverRef.current !== null ||
        ghostsRef.current.length > 0;
      raf = keep ? requestAnimationFrame(loop) : 0;
    };
    raf = requestAnimationFrame(loop);
    return () => {
      if (raf) cancelAnimationFrame(raf);
    };
  }, [active, tick, frozen]);

  return (
    <div className="dl-ghost-layer" ref={rootRef} aria-hidden>
      {ghosts.map((g) => (
        <div
          key={g.id}
          className="dl-ghost"
          style={
            {
              "--tone": g.tone,
              width: g.w,
              height: g.h,
              transform: `translate(${g.x}px, ${g.y}px)`,
              opacity: g.on ? 1 : 0,
            } as CSSProperties
          }
        />
      ))}
    </div>
  );
}
