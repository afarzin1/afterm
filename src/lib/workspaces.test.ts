import { describe, expect, it } from "vitest";
import {
  activateWorkspaceTab,
  activeSessionId,
  addWorkspaceTab,
  createWorkspaceLayout,
  cycleWorkspaceTab,
  focusAdjacentWorkspace,
  focusWorkspace,
  moveWorkspaceTab,
  removeWorkspaceTab,
  resizeWorkspaceSplit,
  splitWorkspace,
  workspaceGeometry,
  workspacePanes,
  workspaceShades,
  zoomGeometry,
  isZoomed
} from "./workspaces";

function ids(...values: string[]) {
  let index = 0;
  return () => values[index++];
}

describe("workspace layout", () => {
  it("peels the active tab into the requested side", () => {
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));

    expect(layout.root).toMatchObject({
      kind: "split",
      orientation: "columns",
      first: { kind: "pane", tabs: ["b", "c"], activeId: "b" },
      second: { kind: "pane", id: "right", tabs: ["a"], activeId: "a" }
    });
    expect(layout.focusedPaneId).toBe("right");
  });

  it("nests vertical and horizontal splits", () => {
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "outer"));
    layout = focusWorkspace(layout, "source");
    layout = splitWorkspace(layout, "down", ids("bottom", "inner"));

    expect(workspacePanes(layout).map(pane => pane.tabs)).toEqual([["c"], ["b"], ["a"]]);
    expect(workspaceGeometry(layout).panes.map(item => item.rect)).toEqual([
      { left: 0, top: 0, width: 0.5, height: 0.5 },
      { left: 0, top: 0.5, width: 0.5, height: 0.5 },
      { left: 0.5, top: 0, width: 0.5, height: 1 }
    ]);
  });

  it("refuses to split the final tab out of a pane", () => {
    const layout = createWorkspaceLayout(["a"], ids("source"));
    expect(splitWorkspace(layout, "right", ids("unused"))).toBe(layout);
  });

  it("collapses an empty pane and focuses its sibling", () => {
    let layout = createWorkspaceLayout(["a", "b"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    layout = removeWorkspaceTab(layout, "a");

    expect(workspacePanes(layout)).toHaveLength(1);
    expect(activeSessionId(layout)).toBe("b");
  });

  it("adds new tabs to the focused pane", () => {
    let layout = createWorkspaceLayout(["a", "b"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    layout = addWorkspaceTab(layout, "c");
    expect(workspacePanes(layout).find(pane => pane.id === "right")?.tabs).toEqual(["a", "c"]);
  });

  it("cycles tabs only inside the focused pane", () => {
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    layout = focusWorkspace(layout, "source");
    layout = cycleWorkspaceTab(layout);

    expect(activeSessionId(layout)).toBe("c");
    expect(workspacePanes(layout).find(pane => pane.id === "right")?.activeId).toBe("a");
  });

  // `Ctrl Shift Tab` is the only caller that steps backwards, and it wraps the
  // same way the forward one does.
  it("cycles tabs backwards, wrapping past the first", () => {
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    layout = focusWorkspace(layout, "source");

    expect(activeSessionId(cycleWorkspaceTab(layout, -1))).toBe("c");
    expect(activeSessionId(cycleWorkspaceTab(cycleWorkspaceTab(layout, -1), -1))).toBe("b");
  });

  it("resizes splits and navigates geometrically", () => {
    let layout = createWorkspaceLayout(["a", "b"], ids("left"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    layout = resizeWorkspaceSplit(layout, "split", 0.7);
    layout = focusWorkspace(layout, "left");

    expect(workspaceGeometry(layout).panes[0].rect.width).toBe(0.7);
    expect(focusAdjacentWorkspace(layout, "right").focusedPaneId).toBe("right");
  });});

describe("dragging a tab along its strip", () => {
  const three = () => createWorkspaceLayout(["a", "b", "c"], ids("pane"));

  it("moves a tab to the index it was dropped on", () => {
    const layout = moveWorkspaceTab(three(), "pane", "a", 2);
    expect(workspacePanes(layout)[0].tabs).toEqual(["b", "c", "a"]);
  });

  it("moves one to the left the same way", () => {
    const layout = moveWorkspaceTab(three(), "pane", "c", 0);
    expect(workspacePanes(layout)[0].tabs).toEqual(["c", "a", "b"]);
  });

  /// Which tab is showing is not what a drag is about, and a drag that ended
  /// on another tab's ground must not bring the pane with it.
  it("leaves the active tab and the focus where they were", () => {
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("pane"));
    layout = activateWorkspaceTab(layout, "pane", "b");
    const moved = moveWorkspaceTab(layout, "pane", "c", 0);
    expect(workspacePanes(moved)[0].activeId).toBe("b");
    expect(moved.focusedPaneId).toBe(layout.focusedPaneId);
  });

  /// Every pointer move asks for one of these, and all but a few land where
  /// the tab already is. The same layout back is what keeps that free.
  it("gives the same layout back when nothing moves", () => {
    const layout = three();
    expect(moveWorkspaceTab(layout, "pane", "b", 1)).toBe(layout);
    expect(moveWorkspaceTab(layout, "pane", "b", 9)).not.toBe(layout);
    expect(moveWorkspaceTab(layout, "pane", "missing", 0)).toBe(layout);
    expect(moveWorkspaceTab(layout, "nowhere", "b", 0)).toBe(layout);
  });

  /// A drag past the end of the strip reads as "last", not as an index off it.
  it("clamps a drop beyond either end", () => {
    expect(workspacePanes(moveWorkspaceTab(three(), "pane", "a", 99))[0].tabs)
      .toEqual(["b", "c", "a"]);
    expect(workspacePanes(moveWorkspaceTab(three(), "pane", "c", -4))[0].tabs)
      .toEqual(["c", "a", "b"]);
  });

  /// The strip a tab is dragged in is its own. A pane is named on the way in,
  /// so a tab that lives somewhere else is not quietly moved there.
  it("will not move a tab that belongs to another pane", () => {
    let layout = createWorkspaceLayout(["a", "b"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    expect(moveWorkspaceTab(layout, "source", "a", 0)).toBe(layout);
  });
});

describe("shades", () => {
  /** Shade per pane, in the order the panes are laid out. */
  const shades = (layout: Parameters<typeof workspaceShades>[0]) => {
    const map = workspaceShades(layout);
    return workspacePanes(layout).map(pane => map.get(pane.id));
  };

  it("paints a lone workspace on the plain ground", () => {
    // Nothing to contrast with, so nothing is darkened and the window reads as
    // one surface — which is the whole point of dropping the border.
    expect(shades(createWorkspaceLayout(["a"]))).toEqual([0]);
  });

  it("alternates the two halves of a split", () => {
    let layout = createWorkspaceLayout(["a", "b"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    expect(shades(layout)).toEqual([0, 1]);
  });

  it("falls out as a checkerboard on a two-by-two grid", () => {
    // Built as two columns that each split downwards, so every pane has a
    // neighbour across and a neighbour below. The diagonal pairs touch at a
    // point rather than along a seam, so they are free to repeat.
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "columns"));
    layout = addWorkspaceTab(layout, "d");
    layout = splitWorkspace(layout, "down", ids("rightDown", "rightRows"));
    layout = focusWorkspace(layout, "source");
    layout = splitWorkspace(layout, "down", ids("sourceDown", "sourceRows"));

    const map = workspaceShades(layout);
    expect(map.get("source")).toBe(0);
    expect(map.get("right")).toBe(1);
    expect(map.get("sourceDown")).toBe(1);
    expect(map.get("rightDown")).toBe(0);
  });

  it("gives a third ground to a pane lying against both halves of a split", () => {
    // The layout two shades could not do: the left pane runs the full height
    // and shares a seam with each half of the right column, and those two
    // share a seam with each other. Three panes, three grounds, no two that
    // touch alike.
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("left"));
    layout = splitWorkspace(layout, "right", ids("right", "columns"));
    layout = addWorkspaceTab(layout, "d");
    layout = splitWorkspace(layout, "down", ids("rightDown", "rightRows"));

    const map = workspaceShades(layout);
    expect(map.get("left")).toBe(0);
    expect(new Set([map.get("left"), map.get("right"), map.get("rightDown")]).size).toBe(3);
  });

  it("never gives two panes that share a seam the same ground", () => {
    // The rule itself, checked against the geometry rather than against a
    // remembered set of answers: build a column beside a stack beside a stack
    // and assert the property every shade exists to hold.
    let layout = createWorkspaceLayout(["a", "b", "c", "d", "e"], ids("left"));
    layout = splitWorkspace(layout, "right", ids("right", "columns"));
    layout = addWorkspaceTab(layout, "f");
    layout = splitWorkspace(layout, "down", ids("rightDown", "rightRows"));
    layout = focusWorkspace(layout, "left");
    layout = splitWorkspace(layout, "down", ids("leftDown", "leftRows"));
    layout = splitWorkspace(layout, "right", ids("leftDownRight", "leftDownColumns"));

    const map = workspaceShades(layout);
    const { panes } = workspaceGeometry(layout);
    expect(panes.length).toBeGreaterThan(3);
    for (const a of panes) {
      for (const b of panes) {
        if (a.pane.id === b.pane.id) continue;
        const seam = a.rect.left + a.rect.width === b.rect.left
          && Math.min(a.rect.top + a.rect.height, b.rect.top + b.rect.height)
            > Math.max(a.rect.top, b.rect.top);
        const stacked = a.rect.top + a.rect.height === b.rect.top
          && Math.min(a.rect.left + a.rect.width, b.rect.left + b.rect.width)
            > Math.max(a.rect.left, b.rect.left);
        if (seam || stacked) {
          expect([a.pane.id, b.pane.id, map.get(a.pane.id) === map.get(b.pane.id)])
            .toEqual([a.pane.id, b.pane.id, false]);
        }
      }
    }
  });

  it("shades by geometry, not by the shape of the tree", () => {
    // The same three panes in a row, nested the other way. A rule counting
    // steps into a `second` child gives 0, 1, 1 here and 0, 1, 0 for the
    // right-nested build, for layouts that look identical on screen.
    let layout = createWorkspaceLayout(["a", "b", "c"], ids("source"));
    layout = splitWorkspace(layout, "right", ids("right", "outer"));
    layout = focusWorkspace(layout, "source");
    layout = splitWorkspace(layout, "right", ids("middle", "inner"));

    const geometry = workspaceGeometry(layout);
    const map = workspaceShades(layout);
    const across = [...geometry.panes]
      .sort((a, b) => a.rect.left - b.rect.left)
      .map(item => map.get(item.pane.id));
    expect(across).toEqual([0, 1, 0]);
  });

  it("has nothing to shade when there is no layout at all", () => {
    expect(workspaceShades({ root: null, focusedPaneId: null }).size).toBe(0);
  });
});

describe("zooming a workspace", () => {
  const twoPanes = () => {
    let layout = createWorkspaceLayout(["a", "b"], ids("left"));
    layout = splitWorkspace(layout, "right", ids("right", "split"));
    return layout;
  };

  it("fills the window with the focused pane and keeps the others, hidden", () => {
    const layout = twoPanes();
    const zoomed = zoomGeometry(layout, workspaceGeometry(layout), "right");
    expect(zoomed.dividers).toEqual([]);
    expect(zoomed.panes).toHaveLength(2);
    const right = zoomed.panes.find(item => item.pane.id === "right")!;
    expect(right.rect).toEqual({ left: 0, top: 0, width: 1, height: 1 });
    expect(right.hidden).toBeUndefined();
    // Still there, so its terminals stay mounted and keep their scrollback.
    expect(zoomed.panes.find(item => item.pane.id === "left")!.hidden).toBe(true);
  });

  it("ends when focus moves to another workspace", () => {
    const layout = focusAdjacentWorkspace(twoPanes(), "left");
    expect(isZoomed(layout, "right")).toBe(false);
    expect(zoomGeometry(layout, workspaceGeometry(layout), "right").dividers).toHaveLength(1);
  });

  it("ends when the zoomed workspace is closed, or a split makes a new one", () => {
    let layout = twoPanes();
    expect(isZoomed(layout, "right")).toBe(true);
    layout = removeWorkspaceTab(layout, "a");
    expect(isZoomed(layout, "right")).toBe(false);

    let again = twoPanes();
    again = addWorkspaceTab(again, "c");
    again = splitWorkspace(again, "down", ids("below", "inner"));
    expect(isZoomed(again, "right")).toBe(false);
  });

  it("has nothing to do with one workspace", () => {
    const layout = createWorkspaceLayout(["a"], ids("only"));
    expect(isZoomed(layout, "only")).toBe(false);
    expect(zoomGeometry(layout, workspaceGeometry(layout), "only").panes[0].hidden).toBeUndefined();
  });
});
