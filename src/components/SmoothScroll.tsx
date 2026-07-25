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
  /** Fires whenever the scroll offset or content size changes. */
  onScroll?: () => void;
}

/**
 * A natively-scrolling list with a custom themed scrollbar in its own column and
 * a pinned header floated over the top.
 *
 * The viewport scrolls natively — no momentum/eased transform, which felt laggy.
 * The native scrollbar is hidden; a custom thumb in `.ss-bar` mirrors the scroll
 * position and can be dragged. The viewport clips top/bottom (so glow can't bleed
 * over the header/statusbar) but leaves a little side room so a highlighted row's
 * glow can spill left/right.
 *
 * Because the header is floated rather than `position: sticky`, the content is
 * padded down by the header's height so the first row rests just below it.
 *
 * The forwarded ref points at the scrolling viewport (for measuring/hit-testing
 * rows and reading its scroll offset).
 */
export const SmoothScroll = forwardRef<HTMLDivElement, Props>(
  function SmoothScroll({ children, className, header, onScroll }, ref) {
    const viewRef = useRef<HTMLDivElement>(null); // native scroll viewport
    const contentRef = useRef<HTMLDivElement>(null);
    const headerRef = useRef<HTMLDivElement>(null);
    const thumbRef = useRef<HTMLDivElement>(null);
    const onScrollRef = useRef(onScroll);
    onScrollRef.current = onScroll;
    useImperativeHandle(ref, () => viewRef.current as HTMLDivElement, []);

    useEffect(() => {
      const view = viewRef.current;
      const content = contentRef.current;
      const thumb = thumbRef.current;
      if (!view || !content || !thumb) return;

      // Size + place the custom thumb from the viewport's real scroll offset.
      const paintThumb = () => {
        const viewH = view.clientHeight;
        const total = view.scrollHeight;
        if (total <= viewH + 1) {
          thumb.style.opacity = "0";
          thumb.style.pointerEvents = "none";
          return;
        }
        thumb.style.opacity = "1";
        thumb.style.pointerEvents = "auto";
        const thumbH = Math.max(32, (viewH / total) * viewH);
        const maxTop = viewH - thumbH;
        const max = total - viewH;
        const top = max > 0 ? (view.scrollTop / max) * maxTop : 0;
        thumb.style.height = `${thumbH}px`;
        thumb.style.transform = `translateY(${top}px)`;
      };

      const onScrollEv = () => {
        paintThumb();
        onScrollRef.current?.();
      };

      // Drag the custom thumb to scroll the native viewport.
      const onThumbDown = (e: MouseEvent) => {
        e.preventDefault();
        document.body.style.userSelect = "none";
        const startY = e.clientY;
        const startScroll = view.scrollTop;
        const viewH = view.clientHeight;
        const total = view.scrollHeight;
        const max = total - viewH;
        const thumbH = Math.max(32, (viewH / total) * viewH);
        const maxTop = viewH - thumbH;
        const onMove = (ev: MouseEvent) => {
          const ratio = maxTop > 0 ? (ev.clientY - startY) / maxTop : 0;
          view.scrollTop = Math.max(0, Math.min(max, startScroll + ratio * max));
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
      // (plus a small gap), then repaint the thumb for the new content size.
      const HEADER_GAP = 12;
      const recompute = () => {
        const headerH = headerRef.current?.offsetHeight ?? 0;
        content.style.paddingTop = headerH ? `${headerH + HEADER_GAP}px` : "0px";
        paintThumb();
      };

      view.addEventListener("scroll", onScrollEv, { passive: true });
      thumb.addEventListener("mousedown", onThumbDown);
      // Content resizes on add/remove and while a card expands; the frame on
      // window resize. Either changes how far we can scroll.
      const ro = new ResizeObserver(recompute);
      ro.observe(view);
      ro.observe(content);
      if (headerRef.current) ro.observe(headerRef.current);
      recompute();

      return () => {
        view.removeEventListener("scroll", onScrollEv);
        thumb.removeEventListener("mousedown", onThumbDown);
        ro.disconnect();
      };
    }, []);

    return (
      <div className={`ss${className ? ` ${className}` : ""}`}>
        <div className="ss-view" ref={viewRef}>
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
