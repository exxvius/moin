// Cursor-driven border glow, CSS-var driven so the styling lives in CSS.
//
// Every card (and download row) gets --mx/--my (pointer position relative to the
// element) plus --glow (0..1 proximity by distance to its nearest edge). The
// edge facing the cursor lights up even when the pointer is outside it — no
// :hover needed. One passive pointermove listener, rAF-throttled; reads all
// rects before writing any vars to avoid layout thrashing.

const GLOW_SELECTOR = ".card, .dl-card";
const GLOW_RANGE = 260; // px falloff: elements within this of the cursor glow

export function initCursorFx(): () => void {
  let frame = 0;
  let clientX = 0;
  let clientY = 0;

  const apply = () => {
    frame = 0;
    const cards = Array.from(
      document.querySelectorAll<HTMLElement>(GLOW_SELECTOR),
    );
    // Read phase: all layout reads first, then the write phase (no interleaving).
    const rects = cards.map((c) => c.getBoundingClientRect());
    cards.forEach((card, i) => {
      const r = rects[i];
      if (r.width === 0 || r.height === 0) return;
      const nx = Math.max(r.left, Math.min(clientX, r.right));
      const ny = Math.max(r.top, Math.min(clientY, r.bottom));
      const dist = Math.hypot(clientX - nx, clientY - ny);
      const glow = Math.max(0, 1 - dist / GLOW_RANGE);
      card.style.setProperty("--mx", `${clientX - r.left}px`);
      card.style.setProperty("--my", `${clientY - r.top}px`);
      card.style.setProperty("--glow", glow.toFixed(3));
    });
  };

  const onMove = (e: PointerEvent) => {
    clientX = e.clientX;
    clientY = e.clientY;
    if (!frame) frame = requestAnimationFrame(apply);
  };

  // When the pointer leaves the window, fade every glow out.
  const onLeave = () => {
    document
      .querySelectorAll<HTMLElement>(GLOW_SELECTOR)
      .forEach((c) => c.style.setProperty("--glow", "0"));
  };

  window.addEventListener("pointermove", onMove, { passive: true });
  document.addEventListener("mouseleave", onLeave);

  return () => {
    window.removeEventListener("pointermove", onMove);
    document.removeEventListener("mouseleave", onLeave);
    if (frame) cancelAnimationFrame(frame);
  };
}
