export type SplitDirection = "left" | "right" | "up" | "down";
export type SplitOrientation = "columns" | "rows";

export interface WorkspacePane {
  kind: "pane";
  id: string;
  tabs: string[];
  activeId: string;
}

export interface WorkspaceSplit {
  kind: "split";
  id: string;
  orientation: SplitOrientation;
  ratio: number;
  first: WorkspaceNode;
  second: WorkspaceNode;
}

export type WorkspaceNode = WorkspacePane | WorkspaceSplit;

export interface WorkspaceLayout {
  root: WorkspaceNode | null;
  focusedPaneId: string | null;
}

export interface WorkspaceRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface PaneGeometry {
  pane: WorkspacePane;
  rect: WorkspaceRect;
  /** Behind a zoomed pane: kept, so its terminals and scrollback survive, but
   *  not drawn. See `zoomGeometry`. */
  hidden?: boolean;
}

export interface DividerGeometry {
  id: string;
  orientation: SplitOrientation;
  ratio: number;
  rect: WorkspaceRect;
}

export interface WorkspaceGeometry {
  panes: PaneGeometry[];
  dividers: DividerGeometry[];
}

const newId = () => crypto.randomUUID();
const clampRatio = (ratio: number) => Math.max(0.1, Math.min(0.9, ratio));
/** Two edges closer than this are the same edge, arrived at by different sums. */
const EDGE_EPSILON = 1e-6;

export function createWorkspaceLayout(
  tabs: string[],
  createId: () => string = newId
): WorkspaceLayout {
  if (tabs.length === 0) return { root: null, focusedPaneId: null };
  const id = createId();
  return {
    root: { kind: "pane", id, tabs: [...tabs], activeId: tabs[0] },
    focusedPaneId: id
  };
}

export function workspacePanes(layout: WorkspaceLayout): WorkspacePane[] {
  const panes: WorkspacePane[] = [];
  const visit = (node: WorkspaceNode | null) => {
    if (!node) return;
    if (node.kind === "pane") panes.push(node);
    else { visit(node.first); visit(node.second); }
  };
  visit(layout.root);
  return panes;
}

export function focusedPane(layout: WorkspaceLayout): WorkspacePane | null {
  return workspacePanes(layout).find(pane => pane.id === layout.focusedPaneId) ?? null;
}

export function activeSessionId(layout: WorkspaceLayout): string | null {
  return focusedPane(layout)?.activeId ?? null;
}

export function paneForSession(layout: WorkspaceLayout, sessionId: string): WorkspacePane | null {
  return workspacePanes(layout).find(pane => pane.tabs.includes(sessionId)) ?? null;
}

function mapPane(
  node: WorkspaceNode,
  paneId: string,
  change: (pane: WorkspacePane) => WorkspaceNode
): WorkspaceNode {
  if (node.kind === "pane") return node.id === paneId ? change(node) : node;
  return {
    ...node,
    first: mapPane(node.first, paneId, change),
    second: mapPane(node.second, paneId, change)
  };
}

export function focusWorkspace(layout: WorkspaceLayout, paneId: string): WorkspaceLayout {
  if (!workspacePanes(layout).some(pane => pane.id === paneId)) return layout;
  if (layout.focusedPaneId === paneId) return layout;
  return { ...layout, focusedPaneId: paneId };
}

export function activateWorkspaceTab(
  layout: WorkspaceLayout,
  paneId: string,
  sessionId: string
): WorkspaceLayout {
  const pane = workspacePanes(layout).find(item => item.id === paneId);
  if (!pane?.tabs.includes(sessionId) || !layout.root) return layout;
  return {
    root: mapPane(layout.root, paneId, current => ({ ...current, activeId: sessionId })),
    focusedPaneId: paneId
  };
}

export function addWorkspaceTab(
  layout: WorkspaceLayout,
  sessionId: string,
  createId: () => string = newId
): WorkspaceLayout {
  const existing = paneForSession(layout, sessionId);
  if (existing) return activateWorkspaceTab(layout, existing.id, sessionId);
  if (!layout.root) return createWorkspaceLayout([sessionId], createId);
  const pane = focusedPane(layout) ?? workspacePanes(layout)[0];
  return {
    root: mapPane(layout.root, pane.id, current => ({
      ...current,
      tabs: [...current.tabs, sessionId],
      activeId: sessionId
    })),
    focusedPaneId: pane.id
  };
}

/**
 * Put a tab at a given index in its own pane's strip.
 *
 * The index is where the tab should end up once it has been taken out, which
 * is what a drag computes and what `dropIndex` returns.
 *
 * Returning the layout itself when nothing moves is not an optimisation but
 * the thing that makes a drag cheap: this is called on every pointer move, and
 * an unchanged reference is a render React does not do and a set of terminals
 * that are not reconciled for a mouse that has travelled four pixels.
 */
export function moveWorkspaceTab(
  layout: WorkspaceLayout,
  paneId: string,
  sessionId: string,
  index: number
): WorkspaceLayout {
  const pane = workspacePanes(layout).find(item => item.id === paneId);
  if (!pane || !layout.root) return layout;
  const from = pane.tabs.indexOf(sessionId);
  if (from < 0) return layout;
  const to = Math.max(0, Math.min(pane.tabs.length - 1, index));
  if (to === from) return layout;
  const tabs = pane.tabs.slice();
  tabs.splice(from, 1);
  tabs.splice(to, 0, sessionId);
  return { ...layout, root: mapPane(layout.root, paneId, current => ({ ...current, tabs })) };
}

export function cycleWorkspaceTab(layout: WorkspaceLayout, step = 1): WorkspaceLayout {
  const pane = focusedPane(layout);
  if (!pane || pane.tabs.length < 2) return layout;
  const index = pane.tabs.indexOf(pane.activeId);
  const next = (index + step + pane.tabs.length) % pane.tabs.length;
  return activateWorkspaceTab(layout, pane.id, pane.tabs[next]);
}

export function splitWorkspace(
  layout: WorkspaceLayout,
  direction: SplitDirection,
  createId: () => string = newId
): WorkspaceLayout {
  const pane = focusedPane(layout);
  if (!layout.root || !pane || pane.tabs.length < 2) return layout;

  const moved = pane.activeId;
  const movedIndex = pane.tabs.indexOf(moved);
  const remaining = pane.tabs.filter(id => id !== moved);
  const source: WorkspacePane = {
    ...pane,
    tabs: remaining,
    activeId: remaining[Math.min(movedIndex, remaining.length - 1)]
  };
  const destination: WorkspacePane = {
    kind: "pane",
    id: createId(),
    tabs: [moved],
    activeId: moved
  };
  const before = direction === "left" || direction === "up";
  const split: WorkspaceSplit = {
    kind: "split",
    id: createId(),
    orientation: direction === "left" || direction === "right" ? "columns" : "rows",
    ratio: 0.5,
    first: before ? destination : source,
    second: before ? source : destination
  };

  return {
    root: mapPane(layout.root, pane.id, () => split),
    focusedPaneId: destination.id
  };
}

function containsPane(node: WorkspaceNode, paneId: string): boolean {
  if (node.kind === "pane") return node.id === paneId;
  return containsPane(node.first, paneId) || containsPane(node.second, paneId);
}

function firstPaneId(node: WorkspaceNode): string {
  return node.kind === "pane" ? node.id : firstPaneId(node.first);
}

function siblingPaneId(node: WorkspaceNode, paneId: string): string | null {
  if (node.kind === "pane") return null;
  if (containsPane(node.first, paneId)) {
    if (node.first.kind === "pane" && node.first.id === paneId) return firstPaneId(node.second);
    return siblingPaneId(node.first, paneId);
  }
  if (containsPane(node.second, paneId)) {
    if (node.second.kind === "pane" && node.second.id === paneId) return firstPaneId(node.first);
    return siblingPaneId(node.second, paneId);
  }
  return null;
}

function removeEmptyPane(node: WorkspaceNode, paneId: string): WorkspaceNode | null {
  if (node.kind === "pane") return node.id === paneId ? null : node;
  const first = removeEmptyPane(node.first, paneId);
  const second = removeEmptyPane(node.second, paneId);
  if (!first) return second;
  if (!second) return first;
  return { ...node, first, second };
}

export function removeWorkspaceTab(layout: WorkspaceLayout, sessionId: string): WorkspaceLayout {
  const pane = paneForSession(layout, sessionId);
  if (!pane || !layout.root) return layout;
  const remaining = pane.tabs.filter(id => id !== sessionId);

  if (remaining.length > 0) {
    const oldIndex = pane.tabs.indexOf(sessionId);
    const activeId = pane.activeId === sessionId
      ? remaining[Math.min(oldIndex, remaining.length - 1)]
      : pane.activeId;
    return {
      ...layout,
      root: mapPane(layout.root, pane.id, current => ({ ...current, tabs: remaining, activeId }))
    };
  }

  const fallback = siblingPaneId(layout.root, pane.id);
  const root = removeEmptyPane(layout.root, pane.id);
  if (!root) return { root: null, focusedPaneId: null };
  const focusedStillExists = layout.focusedPaneId !== pane.id
    && workspacePanes({ root, focusedPaneId: layout.focusedPaneId }).some(
      item => item.id === layout.focusedPaneId
    );
  return {
    root,
    focusedPaneId: focusedStillExists ? layout.focusedPaneId : fallback ?? firstPaneId(root)
  };
}

export function resizeWorkspaceSplit(
  layout: WorkspaceLayout,
  splitId: string,
  ratio: number
): WorkspaceLayout {
  const resize = (node: WorkspaceNode): WorkspaceNode => {
    if (node.kind === "pane") return node;
    if (node.id === splitId) return { ...node, ratio: clampRatio(ratio) };
    return { ...node, first: resize(node.first), second: resize(node.second) };
  };
  return layout.root ? { ...layout, root: resize(layout.root) } : layout;
}

export function workspaceGeometry(layout: WorkspaceLayout): WorkspaceGeometry {
  const panes: PaneGeometry[] = [];
  const dividers: DividerGeometry[] = [];
  const visit = (node: WorkspaceNode, rect: WorkspaceRect) => {
    if (node.kind === "pane") {
      panes.push({ pane: node, rect });
      return;
    }
    const ratio = clampRatio(node.ratio);
    dividers.push({ id: node.id, orientation: node.orientation, ratio, rect });
    if (node.orientation === "columns") {
      visit(node.first, { ...rect, width: rect.width * ratio });
      visit(node.second, {
        left: rect.left + rect.width * ratio,
        top: rect.top,
        width: rect.width * (1 - ratio),
        height: rect.height
      });
    } else {
      visit(node.first, { ...rect, height: rect.height * ratio });
      visit(node.second, {
        left: rect.left,
        top: rect.top + rect.height * ratio,
        width: rect.width,
        height: rect.height * (1 - ratio)
      });
    }
  };
  if (layout.root) visit(layout.root, { left: 0, top: 0, width: 1, height: 1 });
  return { panes, dividers };
}

/**
 * How many grounds a pane can stand on, which is how many shades there are to
 * hand out. Matched by the theme's `grounds`, and asserted there.
 */
export const SHADE_COUNT = 3;

/**
 * Do these two rectangles share an edge, rather than merely a corner?
 *
 * Touching at a corner is not touching for this purpose: two panes diagonal to
 * each other in a grid meet at a point, and a point cannot be seen to be a
 * seam. Requiring the shared span to have positive length is what tells the
 * two apart.
 */
function touching(a: WorkspaceRect, b: WorkspaceRect): boolean {
  const meets = (aStart: number, aSize: number, bStart: number, bSize: number) =>
    Math.abs(aStart + aSize - bStart) <= EDGE_EPSILON
    || Math.abs(bStart + bSize - aStart) <= EDGE_EPSILON;
  const overlaps = (aStart: number, aSize: number, bStart: number, bSize: number) =>
    Math.min(aStart + aSize, bStart + bSize) - Math.max(aStart, bStart) > EDGE_EPSILON;

  if (meets(a.left, a.width, b.left, b.width)) {
    return overlaps(a.top, a.height, b.top, b.height);
  }
  if (meets(a.top, a.height, b.top, b.height)) {
    return overlaps(a.left, a.width, b.left, b.width);
  }
  return false;
}

/**
 * Which ground each pane paints on, keyed by pane id.
 *
 * Panes are told apart by shade rather than by a line drawn round them, so the
 * rule has to be the one the eye applies: two panes that share an edge should
 * differ. That is a graph colouring — a vertex per pane, an edge wherever two
 * rectangles touch along a seam of real length — and it is solved here by
 * taking the panes in reading order and giving each the lowest shade none of
 * its already-coloured neighbours has.
 *
 * It used to be the parity of a pane's column plus its row, a checkerboard,
 * and a checkerboard has only two colours to give out. Two is not enough. A
 * pane beside a split — `A | [B1 / B2]` — touches both halves of it, and those
 * two halves must differ from each other, so with two shades one of them is
 * forced to match A: three panes meeting at a point, with only two grounds
 * between them, and two of them sharing a seam and a colour. That is the case
 * this used to get wrong, and a third ground is exactly what it needs.
 *
 * Reading order rather than tree order for the same reason the checkerboard
 * was taken from rectangles rather than from splits: three panes in a row are
 * one layout on screen however the splits happened to nest, and they should be
 * shaded the same way either way.
 *
 * Deriving it from rectangles also keeps the top-left pane on shade 0 — it is
 * first in reading order and so has no coloured neighbour to avoid. The pane
 * against the sidebar is therefore always the ground the sidebar is not, and
 * that edge keeps its contrast whatever the layout does.
 *
 * Three shades still cannot be a guarantee: four rectangles can in principle
 * be mutually adjacent, and greedy colouring is not optimal even when a
 * solution exists. Both are unreachable from splitting the way afterm splits,
 * and neither is worth an exact search over four panes. When a pane's
 * neighbours have taken every shade, it takes the one fewest of them are
 * using, which puts the one unavoidable repeat against the shortest seam
 * available rather than wherever the loop happened to be.
 */
export function workspaceShades(layout: WorkspaceLayout): Map<string, number> {
  const order = [...workspaceGeometry(layout).panes]
    .sort((a, b) => a.rect.top - b.rect.top || a.rect.left - b.rect.left);

  const shades = new Map<string, number>();
  for (const item of order) {
    const neighbours = new Array<number>(SHADE_COUNT).fill(0);
    for (const other of order) {
      const shade = shades.get(other.pane.id);
      if (shade !== undefined && touching(item.rect, other.rect)) neighbours[shade] += 1;
    }
    let best = 0;
    for (let shade = 1; shade < SHADE_COUNT; shade += 1) {
      if (neighbours[shade] < neighbours[best]) best = shade;
    }
    shades.set(item.pane.id, best);
  }
  return shades;
}

export function focusAdjacentWorkspace(
  layout: WorkspaceLayout,
  direction: SplitDirection
): WorkspaceLayout {
  const geometry = workspaceGeometry(layout);
  const current = geometry.panes.find(item => item.pane.id === layout.focusedPaneId);
  if (!current) return layout;
  const cx = current.rect.left + current.rect.width / 2;
  const cy = current.rect.top + current.rect.height / 2;
  const candidates = geometry.panes
    .filter(item => item.pane.id !== current.pane.id)
    .map(item => {
      const x = item.rect.left + item.rect.width / 2;
      const y = item.rect.top + item.rect.height / 2;
      const primary = direction === "left" ? cx - x
        : direction === "right" ? x - cx
          : direction === "up" ? cy - y : y - cy;
      const secondary = direction === "left" || direction === "right"
        ? Math.abs(y - cy) : Math.abs(x - cx);
      return { item, primary, score: primary * 10 + secondary };
    })
    .filter(candidate => candidate.primary > 0)
    .sort((a, b) => a.score - b.score);
  return candidates[0] ? focusWorkspace(layout, candidates[0].item.pane.id) : layout;
}

/**
 * Whether a workspace can be zoomed: there has to be something to zoom past.
 */
export function canZoom(layout: WorkspaceLayout): boolean {
  return workspacePanes(layout).length > 1;
}

/**
 * The layout with one workspace filling the window.
 *
 * Zoom is held outside the layout, as the id of the pane that was focused when
 * it was asked for, and it counts only while that pane still has focus and
 * there is more than one. That one rule is what ends it at the right moments
 * without every layout operation having to know about it: moving focus to a
 * neighbour, splitting — which focuses the new pane — and closing the zoomed
 * pane all leave the id pointing somewhere it no longer applies.
 *
 * The other panes stay in the geometry, marked hidden, rather than being left
 * out. A pane that is not placed has its terminals unmounted, and an unmounted
 * terminal is a lost scrollback; hidden, they keep running behind the zoom and
 * come back as they were.
 */
export function zoomGeometry(
  layout: WorkspaceLayout,
  geometry: WorkspaceGeometry,
  zoomed: string | null
): WorkspaceGeometry {
  if (!zoomed || zoomed !== layout.focusedPaneId || !canZoom(layout)) return geometry;
  if (!geometry.panes.some(item => item.pane.id === zoomed)) return geometry;
  return {
    panes: geometry.panes.map(item => item.pane.id === zoomed
      ? { ...item, rect: { left: 0, top: 0, width: 1, height: 1 } }
      : { ...item, hidden: true }),
    dividers: []
  };
}

/** Whether zoom is in effect, by the same rule `zoomGeometry` applies. */
export function isZoomed(layout: WorkspaceLayout, zoomed: string | null): boolean {
  return zoomed !== null && zoomed === layout.focusedPaneId && canZoom(layout);
}
