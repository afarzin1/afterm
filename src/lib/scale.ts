/**
 * How much larger than drawn the chrome should be rendered on a dense display.
 *
 * The file pane and the status bar were sized against a 1080p panel, and a CSS
 * pixel is not a fixed physical size. On a 4K panel run at 100% it covers a
 * quarter of the area it covers at 1080p, so type laid out in it comes out at
 * half the height it was designed at, which is the size complaint.
 *
 * The signal is `screen.width` rather than the window's, and rather than a CSS
 * media query, for two reasons:
 *
 * - It is the *display* the text is physically small on. A half-width window
 *   on a 4K panel needs exactly the treatment a maximised one does, and a
 *   width media query would give it the 1080p sizes.
 * - It is already in CSS pixels, so Windows' own display scaling has been
 *   applied to it before afterm sees it. A 4K panel at 200% reports 1920 and
 *   is left alone — Windows has already done this job, and scaling again on
 *   top of it would double-count.
 */

/// Lower bound in CSS pixels, and the scale a display at least that wide gets.
/// Ordered widest first, so the first match is the right one.
const STEPS: readonly (readonly [number, number])[] = [
  [3400, 1.35], // 4K at 100%, which reports 3840
  [2400, 1.18], // 1440p at 100%, or 4K at 150%, which report 2560
  [0, 1]        // 1080p, and anything Windows has already scaled down to it
] as const;

/** The scale for a display this many CSS pixels wide. */
export function scaleForWidth(width: number): number {
  return STEPS.find(([from]) => width >= from)?.[1] ?? 1;
}

/**
 * Set `--ui-scale` from the current display, and keep it set.
 *
 * Returns the unsubscribe. `resize` is the signal because it fires when the
 * window is dragged to a monitor with a different scaling factor; without it
 * the scale would be stuck at whichever display the window opened on.
 */
export function trackDisplayScale(): () => void {
  const apply = () => {
    document.documentElement.style.setProperty(
      "--ui-scale",
      String(scaleForWidth(window.screen.width))
    );
  };
  apply();
  window.addEventListener("resize", apply);
  return () => window.removeEventListener("resize", apply);
}
