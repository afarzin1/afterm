/**
 * Where a dragged tab belongs, given where the pointer is.
 *
 * `centres` is the horizontal midpoint of every *other* tab in the strip, in
 * the order they are drawn; the answer is how many of them the pointer has
 * passed, which is the index the dragged tab should take.
 *
 * Leaving the dragged tab's own centre out of that list is the whole of the
 * rule, and the reason is arithmetic rather than taste. Counting it as well
 * would have a tab that crosses one neighbour's midpoint score two — its own,
 * which it is still sitting on, and the neighbour's — so a drag one place to
 * the right would move it two, and the tab would run ahead of the pointer.
 *
 * The strip is re-measured on every move and the tabs reorder under the
 * pointer as it goes, so the midpoints this reads are always the ones on
 * screen. That also supplies the dead zone a drag needs: nothing moves until
 * the pointer has travelled half a tab, which is a deliberate distance rather
 * than a threshold in pixels someone has to guess at.
 *
 * Kept out of the grid and free of the DOM so the rule can be read and tested
 * on its own, the way `copiesOnRelease` is.
 */
export function dropIndex(centres: number[], x: number): number {
  let index = 0;
  for (const centre of centres) {
    if (x > centre) index += 1;
  }
  return index;
}
