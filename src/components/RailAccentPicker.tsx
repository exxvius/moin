import { useEffect, useRef, type CSSProperties } from "react";
import { ACCENTS, type Accent } from "../lib/accent";

interface Props {
  accent: Accent;
  setAccent: (a: Accent) => void;
}

// The slots rendered around the current accent. Showing ±2 (not just ±1) means a
// fresh swatch is already mounted just off-screen, so scrolling reads as a
// continuous wheel rather than items popping in at the edges.
const WINDOW = [-2, -1, 0, 1, 2];

/** The accent picker in the side rail. Hovering the swatch reveals a vertical
 *  color carousel to its side (previous / current / next) and hides the cursor;
 *  scrolling cycles the theme accent one color at a time; moving away restores
 *  the cursor and closes the strip. */
export function RailAccentPicker({ accent, setAccent }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  // The live index + last-step time, as refs so the (once-attached) wheel handler
  // always reads the current accent and can throttle without re-attaching.
  const idxRef = useRef(0);
  const lastStep = useRef(0);

  const idx = Math.max(
    0,
    ACCENTS.findIndex((a) => a.id === accent),
  );
  idxRef.current = idx;
  // The accent `offset` steps away from the current one (wraps around the wheel).
  const at = (offset: number): (typeof ACCENTS)[number] =>
    ACCENTS[(idx + offset + ACCENTS.length) % ACCENTS.length];

  // Native, non-passive wheel listener so scrolling over the swatch cycles the
  // accent instead of scrolling the page. One color per wheel event, throttled so
  // a mouse notch and a trackpad flick both advance at a controlled pace (rather
  // than jumping several colors per notch when the OS reports a big delta).
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const COOLDOWN = 80;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (Math.abs(e.deltaY) < 1) return;
      const now = e.timeStamp;
      if (now - lastStep.current < COOLDOWN) return;
      lastStep.current = now;
      const dir = e.deltaY > 0 ? 1 : -1;
      idxRef.current = (idxRef.current + dir + ACCENTS.length) % ACCENTS.length;
      setAccent(ACCENTS[idxRef.current].id);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [setAccent]);

  const stepBy = (dir: 1 | -1) =>
    setAccent(ACCENTS[(idx + dir + ACCENTS.length) % ACCENTS.length].id);

  return (
    <div className="rail-accent" ref={ref}>
      <button
        type="button"
        className="rail-accent-swatch"
        aria-label={`Accent color: ${ACCENTS[idx].label}. Scroll or use arrow keys to change.`}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowRight") {
            e.preventDefault();
            stepBy(1);
          } else if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
            e.preventDefault();
            stepBy(-1);
          }
        }}
      >
        <span className="swatch-dot" style={{ background: "var(--accent)" }} />
      </button>

      <div className="rail-accent-pop" aria-hidden>
        <span className="rap-chev up" style={{ color: at(-1).swatch }} />
        <div className="rap-wheel">
          {/* A window of the accent wheel, each keyed by id so it persists across
           *  steps — its offset (and thus position/scale) transitions fluidly as
           *  the current accent changes. */}
          {WINDOW.map((o) => {
            const a = at(o);
            return (
              <span
                key={a.id}
                className="rap-sw"
                data-off={o}
                style={
                  { background: a.swatch, "--o": o } as CSSProperties
                }
              />
            );
          })}
        </div>
        <span className="rap-chev down" style={{ color: at(1).swatch }} />
      </div>
    </div>
  );
}
