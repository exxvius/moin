// Cursor-driven border glow, CSS-var driven so the styling lives in CSS.
//
// Every card (and download row) gets --mx/--my (pointer position relative to the
// element) plus --glow (0..1 proximity by distance to its nearest edge). The
// edge facing the cursor lights up even when the pointer is outside it — no
// :hover needed. One passive pointermove listener, rAF-throttled; reads all
// rects before writing any vars to avoid layout thrashing.
//
// Only cards actually within range get their vars written each frame (the glow
// is a masked radial-gradient whose repaint isn't free), and a marquee/scrollbar
// drag stands the effect down entirely — its per-card repaint was the drag lag.

const GLOW_SELECTOR = ".card, .dl-card, .cat-card";
const GLOW_RANGE = 260; // px falloff: elements within this of the cursor glow

export function initCursorFx(): () => void {
  let frame = 0;
  let clientX = 0;
  let clientY = 0;
  // Cards we wrote a non-zero --glow to last frame, so exactly those that leave
  // range get zeroed — without touching every card in the document each frame.
  let lit = new Set<HTMLElement>();

  const clearLit = () => {
    for (const card of lit) card.style.setProperty("--glow", "0");
    lit = new Set();
  };

  const apply = () => {
    frame = 0;
    // A marquee/scrollbar drag owns the pointer; the proximity glow is just noise
    // then, and repainting every row's gradient ring per frame was the drag lag.
    if (document.body.classList.contains("dragging")) {
      if (lit.size) clearLit();
      return;
    }
    const cards = Array.from(
      document.querySelectorAll<HTMLElement>(GLOW_SELECTOR),
    );
    // Read phase: all layout reads first, then the write phase (no interleaving).
    const rects = cards.map((c) => c.getBoundingClientRect());
    const next = new Set<HTMLElement>();
    cards.forEach((card, i) => {
      const r = rects[i];
      if (r.width === 0 || r.height === 0) return;
      const nx = Math.max(r.left, Math.min(clientX, r.right));
      const ny = Math.max(r.top, Math.min(clientY, r.bottom));
      const dist = Math.hypot(clientX - nx, clientY - ny);
      if (dist >= GLOW_RANGE) return; // out of range: leave it dark (zeroed below)
      const glow = 1 - dist / GLOW_RANGE;
      card.style.setProperty("--mx", `${clientX - r.left}px`);
      card.style.setProperty("--my", `${clientY - r.top}px`);
      card.style.setProperty("--glow", glow.toFixed(3));
      next.add(card);
    });
    // Zero out cards that were lit last frame but have now left range.
    for (const card of lit) if (!next.has(card)) card.style.setProperty("--glow", "0");
    lit = next;
  };

  const onMove = (e: PointerEvent) => {
    clientX = e.clientX;
    clientY = e.clientY;
    if (!frame) frame = requestAnimationFrame(apply);
  };

  // When the pointer leaves the window, fade every glow out.
  const onLeave = () => {
    if (frame) {
      cancelAnimationFrame(frame);
      frame = 0;
    }
    clearLit();
  };

  window.addEventListener("pointermove", onMove, { passive: true });
  document.addEventListener("mouseleave", onLeave);

  return () => {
    window.removeEventListener("pointermove", onMove);
    document.removeEventListener("mouseleave", onLeave);
    if (frame) cancelAnimationFrame(frame);
  };
}
