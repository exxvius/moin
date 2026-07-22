import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  type ReactNode,
} from "react";

interface Props {
  children: ReactNode;
  className?: string;
  /** Pinned, non-scrolling header floated over the top of the list. */
  header?: ReactNode;
  /** Content rendered behind the viewport (e.g. an escaping-glow layer). */
  behind?: ReactNode;
  /** Fires whenever the scroll offset or content size changes. */
  onScroll?: () => void;
}

/**
 * An eased (momentum) scroller with a custom scrollbar in its own column.
 *
 * Unlike a normal scroll container, this one moves its content with a transform
 * inside an `overflow: visible` viewport, and clips vertically with a CSS mask
 * that leaves the sides open. That lets each row's side glow spill past the
 * viewport edges (a real scroll container would clip it on both axes).
 *
 * Because transform-scrolling has no scroll container, `position: sticky` can't
 * pin a header — so the header is rendered separately and floated on top, and
 * the content is padded down by the header's height.
 *
 * The forwarded ref points at the viewport (for measuring/hit-testing rows).
 */
export const SmoothScroll = forwardRef<HTMLDivElement, Props>(
  function SmoothScroll({ children, className, header, behind, onScroll }, ref) {
    const frameRef = useRef<HTMLDivElement>(null); // masked viewport
    const contentRef = useRef<HTMLDivElement>(null); // transformed list
    const headerRef = useRef<HTMLDivElement>(null);
    const thumbRef = useRef<HTMLDivElement>(null);
    const onScrollRef = useRef(onScroll);
    onScrollRef.current = onScroll;
    useImperativeHandle(ref, () => frameRef.current as HTMLDivElement, []);

    useEffect(() => {
      const frame = frameRef.current;
      const content = contentRef.current;
      const thumb = thumbRef.current;
      if (!frame || !content || !thumb) return;

      let target = 0;
      let current = 0;
      let raf = 0;

      const metrics = () => {
        const view = frame.clientHeight;
        const total = content.scrollHeight; // includes the header padding
        return { view, total, max: Math.max(0, total - view) };
      };

      const paintThumb = () => {
        const { view, total, max } = metrics();
        if (total <= view + 1) {
          thumb.style.opacity = "0";
          thumb.style.pointerEvents = "none";
          return;
        }
        thumb.style.opacity = "1";
        thumb.style.pointerEvents = "auto";
        const thumbH = Math.max(32, (view / total) * view);
        const maxTop = view - thumbH;
        const top = max > 0 ? (current / max) * maxTop : 0;
        thumb.style.height = `${thumbH}px`;
        thumb.style.transform = `translateY(${top}px)`;
      };

      const apply = () => {
        content.style.transform = `translateY(${-current}px)`;
        paintThumb();
        onScrollRef.current?.();
      };

      const tick = () => {
        current += (target - current) * 0.16;
        if (Math.abs(target - current) < 0.4) current = target;
        apply();
        raf = current === target ? 0 : requestAnimationFrame(tick);
      };

      const onWheel = (e: WheelEvent) => {
        const { max } = metrics();
        if (max <= 0) return;
        e.preventDefault();
        target = Math.max(0, Math.min(max, target + e.deltaY));
        if (!raf) raf = requestAnimationFrame(tick);
      };

      const onThumbDown = (e: MouseEvent) => {
        e.preventDefault();
        document.body.style.userSelect = "none";
        const startY = e.clientY;
        const startScroll = current;
        const { view, total, max } = metrics();
        const thumbH = Math.max(32, (view / total) * view);
        const maxTop = view - thumbH;
        const onMove = (ev: MouseEvent) => {
          const ratio = maxTop > 0 ? (ev.clientY - startY) / maxTop : 0;
          const next = Math.max(0, Math.min(max, startScroll + ratio * max));
          target = next;
          current = next;
          apply();
        };
        const onUp = () => {
          document.body.style.userSelect = "";
          window.removeEventListener("mousemove", onMove);
          window.removeEventListener("mouseup", onUp);
        };
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
      };

      // Push the list down so the first row rests just below the pinned header
      // (plus a small gap so it doesn't butt against it), then re-clamp in case
      // the content shrank under the current offset.
      const HEADER_GAP = 12;
      const recompute = () => {
        const headerH = headerRef.current?.offsetHeight ?? 0;
        content.style.paddingTop = headerH ? `${headerH + HEADER_GAP}px` : "0px";
        const { max } = metrics();
        current = Math.min(current, max);
        target = Math.min(target, max);
        apply();
      };

      frame.addEventListener("wheel", onWheel, { passive: false });
      thumb.addEventListener("mousedown", onThumbDown);
      // Content resizes on add/remove and while a card expands; the frame on
      // window resize. Either changes how far we can scroll.
      const ro = new ResizeObserver(recompute);
      ro.observe(frame);
      ro.observe(content);
      if (headerRef.current) ro.observe(headerRef.current);
      recompute();

      return () => {
        frame.removeEventListener("wheel", onWheel);
        thumb.removeEventListener("mousedown", onThumbDown);
        ro.disconnect();
        if (raf) cancelAnimationFrame(raf);
      };
    }, []);

    return (
      <div className={`ss${className ? ` ${className}` : ""}`}>
        {behind}
        <div className="ss-view" ref={frameRef}>
          <div className="ss-content" ref={contentRef}>
            {children}
          </div>
        </div>
        {header && (
          <div className="ss-header" ref={headerRef}>
            {header}
          </div>
        )}
        <div className="ss-bar">
          <div className="ss-thumb" ref={thumbRef} />
        </div>
      </div>
    );
  },
);
