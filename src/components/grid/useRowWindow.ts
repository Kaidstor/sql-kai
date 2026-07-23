import {
  useLayoutEffect,
  useState,
  type RefObject,
} from "react";

/** Rows rendered beyond each viewport edge so a small scroll shows real rows
 *  immediately instead of a blank strip while React catches up. */
const OVERSCAN = 12;
/** Positions the very first frame only — replaced by a real measurement. */
const FALLBACK_ROW_H = 24;

/**
 * Windowing of the grid body: only the entries that can be visible in the
 * scroller (plus overscan) are mounted; GridBody adds two spacer rows so the
 * table keeps its full height — the scrollbar and the saved scroll offsets
 * behave exactly as before. The math assumes uniform entry height (GridBody
 * flattens multi-line values to keep that true) and takes the real height
 * from the shortest rendered data row: the minimum, so an odd row inflated by
 * a tall fallback glyph (emoji) can't skew the estimate for the whole table.
 */
export function useRowWindow(
  count: number,
  /** layout.sized — re-measure when the fixed column layout kicks in. */
  sized: boolean,
  scrollRef: RefObject<HTMLDivElement | null>,
  bodyTableRef: RefObject<HTMLTableElement | null>,
) {
  const [rowH, setRowH] = useState(FALLBACK_ROW_H);
  const [firstRow, setFirstRow] = useState(0);
  const [viewH, setViewH] = useState(0);

  // The scroller's height, tracked through window resizes and panel toggles.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    setViewH(el.clientHeight);
    return () => ro.disconnect();
  }, [scrollRef]);

  // Measured per result (and again when the fixed layout kicks in, which is
  // before any editing can start) — a mounted cell editor makes its row a few
  // px taller, so continuous re-measuring would drift. Kept fractional:
  // rounding would accumulate across tens of thousands of rows.
  useLayoutEffect(() => {
    const trs = bodyTableRef.current?.querySelectorAll("tbody tr[data-ri]");
    if (!trs || trs.length === 0) return;
    let h = Infinity;
    trs.forEach((tr) => {
      const trH = tr.getBoundingClientRect().height;
      if (trH > 0 && trH < h) h = trH;
    });
    if (h !== Infinity && Math.abs(h - rowH) > 0.25) setRowH(h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count, sized, bodyTableRef]);

  const onScroll = (scrollTop: number) =>
    setFirstRow(Math.floor(scrollTop / rowH));

  const first = Math.max(0, Math.min(firstRow, count - 1));
  const visible = Math.ceil(viewH / rowH) + 1;
  const start = Math.max(0, first - OVERSCAN);
  const end = Math.min(count, first + visible + OVERSCAN);
  return { start, end, rowH, onScroll };
}
