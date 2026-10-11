import { useMemo, useRef, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import { Bell, CirclePlus, MessageSquare, MessageSquareMore, Minimize2, PanelLeftOpen, X } from "lucide-react";
import { TerminalPane, type PaneActions } from "./TerminalPane";
import type { BackendFailure, Session } from "../types";
import type { Retrying } from "../lib/reconnect";
import type { Notice } from "../lib/attention";
import {
  workspaceGeometry,
  workspaceShades,
  zoomGeometry,
  type DividerGeometry,
  type WorkspaceLayout,
  type WorkspaceRect
} from "../lib/workspaces";
import { dropIndex } from "../lib/reorder";
import type { Theme } from "../theme";

const TAB_BAR_HEIGHT = 35;
const MIN_PANE_WIDTH = 220;
const MIN_PANE_HEIGHT = 140;

interface Props {
  layout: WorkspaceLayout;
  /// The workspace filling the window, if one is; see `zoomGeometry`.
  zoomed: string | null;
  onUnzoom(): void;
  sessions: Session[];
  sidebar: boolean;
  fontSize: number;
  theme: Theme;
  copyOnSelect: boolean;
  renaming: string | null;
  draftName: string;
  onDraftName(value: string): void;
  onCommitRename(): void;
  onCancelRename(): void;
  onStartRename(id: string): void;
  onActivateTab(paneId: string, sessionId: string): void;
  onMoveTab(paneId: string, sessionId: string, index: number): void;
  onFocusPane(paneId: string): void;
  onCloseSession(id: string): void;
  onNewSession(paneId: string): void;
  onOpenSidebar(): void;
  onResizeSplit(id: string, ratio: number): void;
  onReady(id: string): void;
  onExit(id: string, failure: BackendFailure): void;
  /// Sessions being reconnected on their own, for each pane to show its own.
  retrying: ReadonlyMap<string, Retrying>;
  onRetry(id: string): void;
  onStopRetry(id: string): void;
  onQuit(id: string): void;
  onIntegration(id: string, present: boolean): void;
  onDirectory(id: string, path: string): void;
  /// Tabs something has asked for the user in, and what it said.
  attention: ReadonlyMap<string, Notice>;
  onAttention(id: string, notice: Notice): void;
  onActions(id: string, actions: PaneActions | null): void;
  onCopy(): void;
  onNotice(message: string): void;
}

const percentage = (value: number) => `${value * 100}%`;

function frameStyle(rect: WorkspaceRect): CSSProperties {
  return {
    left: percentage(rect.left),
    top: percentage(rect.top),
    width: percentage(rect.width),
    height: percentage(rect.height)
  };
}

function terminalStyle(rect: WorkspaceRect): CSSProperties {
  return {
    left: percentage(rect.left),
    top: `calc(${percentage(rect.top)} + ${TAB_BAR_HEIGHT}px)`,
    width: percentage(rect.width),
    height: `calc(${percentage(rect.height)} - ${TAB_BAR_HEIGHT}px)`
  };
}

function dividerStyle(divider: DividerGeometry): CSSProperties {
  if (divider.orientation === "columns") {
    return {
      left: `calc(${percentage(divider.rect.left + divider.rect.width * divider.ratio)} - 3px)`,
      top: percentage(divider.rect.top),
      width: "6px",
      height: percentage(divider.rect.height)
    };
  }
  return {
    left: percentage(divider.rect.left),
    top: `calc(${percentage(divider.rect.top + divider.rect.height * divider.ratio)} - 3px)`,
    width: percentage(divider.rect.width),
    height: "6px"
  };
}

interface Drag {
  id: string;
  pointerId: number;
  orientation: DividerGeometry["orientation"];
  rect: WorkspaceRect;
}

/** The tab in hand, while one is being dragged along its strip. */
interface TabDrag {
  paneId: string;
  sessionId: string;
  pointerId: number;
}

export function WorkspaceGrid({
  layout,
  zoomed,
  onUnzoom,
  sessions,
  sidebar,
  fontSize,
  theme,
  copyOnSelect,
  renaming,
  draftName,
  onDraftName,
  onCommitRename,
  onCancelRename,
  onStartRename,
  onActivateTab,
  onMoveTab,
  onFocusPane,
  onCloseSession,
  onNewSession,
  onOpenSidebar,
  onResizeSplit,
  onReady,
  onExit,
  retrying,
  onRetry,
  onStopRetry,
  onQuit,
  onIntegration,
  onDirectory,
  attention,
  onAttention,
  onActions,
  onCopy,
  onNotice
}: Props) {
  const host = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const tabDrag = useRef<TabDrag | null>(null);
  const geometry = useMemo(
    () => zoomGeometry(layout, workspaceGeometry(layout), zoomed),
    [layout, zoomed]
  );
  const shown = useMemo(() => geometry.panes.filter(item => !item.hidden), [geometry]);
  const shades = useMemo(() => workspaceShades(layout), [layout]);
  const placement = useMemo(() => {
    const bySession = new Map<string, typeof geometry.panes[number]>();
    for (const item of geometry.panes) {
      for (const sessionId of item.pane.tabs) bySession.set(sessionId, item);
    }
    return bySession;
  }, [geometry]);
  const sessionsById = useMemo(
    () => new Map(sessions.map(session => [session.id, session])),
    [sessions]
  );

  const constrainRatio = (
    orientation: DividerGeometry["orientation"],
    rect: WorkspaceRect,
    ratio: number
  ) => {
    const bounds = host.current?.getBoundingClientRect();
    if (!bounds) return ratio;
    const horizontal = orientation === "columns";
    const total = (horizontal ? bounds.width : bounds.height)
      * (horizontal ? rect.width : rect.height);
    if (total <= 0) return ratio;
    const minimumPixels = horizontal ? MIN_PANE_WIDTH : MIN_PANE_HEIGHT;
    const minimum = Math.min(0.45, minimumPixels / total);
    return Math.max(minimum, Math.min(1 - minimum, ratio));
  };

  const resizeFromPointer = (event: ReactPointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    const bounds = host.current?.getBoundingClientRect();
    if (!current || current.pointerId !== event.pointerId || !bounds) return;
    const horizontal = current.orientation === "columns";
    const total = (horizontal ? bounds.width : bounds.height)
      * (horizontal ? current.rect.width : current.rect.height);
    if (total <= 0) return;
    const origin = (horizontal ? bounds.left : bounds.top)
      + (horizontal ? current.rect.left * bounds.width : current.rect.top * bounds.height);
    const pointer = horizontal ? event.clientX : event.clientY;
    const ratio = constrainRatio(
      current.orientation,
      current.rect,
      (pointer - origin) / total
    );
    onResizeSplit(current.id, ratio);
  };

  /**
   * A tab being dragged along its strip, one pointer move at a time.
   *
   * The strip is measured here rather than remembered from the grab, because
   * the tabs move as the drag goes: each move reorders them, and the next one
   * has to read the order that is now on screen. Measuring is a handful of
   * `getBoundingClientRect` calls on the tabs of one pane, and it is what lets
   * the tab travel with the pointer instead of a ghost of it being drawn.
   *
   * Only this pane's strip. A tab dragged onto another workspace stays where
   * it was — moving a session between panes is a different operation, and one
   * a stray drag should not be able to do by accident.
   */
  const reorderFromPointer = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = tabDrag.current;
    if (!current || current.pointerId !== event.pointerId) return;
    const tab = event.currentTarget;
    const strip = tab.parentElement;
    if (!strip) return;
    const centres: number[] = [];
    for (const other of Array.from(strip.children)) {
      if (other === tab) continue;
      const rect = other.getBoundingClientRect();
      centres.push(rect.left + rect.width / 2);
    }
    onMoveTab(current.paneId, current.sessionId, dropIndex(centres, event.clientX));
  };

  return <div className="workspace-grid" ref={host}>
    {sessions.length === 0 && <div className="terminal-empty">
      <div>
        <h2>No open sessions</h2>
        <ul>
          <li><kbd>Ctrl Shift N</kbd> new session</li>
          <li><kbd>Ctrl Shift Space</kbd> command palette</li>
          <li><kbd>Ctrl Shift B</kbd> toggle sidebar</li>
        </ul>
      </div>
    </div>}

    {/* Kept as one flat keyed list. Moving a tab changes only its rectangle;
        its xterm instance and scrollback never leave this React parent. */}
    {sessions.map(session => {
      const item = placement.get(session.id);
      if (!item) return null;
      const visible = !item.hidden && item.pane.activeId === session.id;
      const focused = visible && layout.focusedPaneId === item.pane.id;
      return <TerminalPane
        key={session.id}
        session={session}
        visible={visible}
        focused={focused}
        style={terminalStyle(item.rect)}
        fontSize={fontSize}
        theme={theme}
        copyOnSelect={copyOnSelect}
        shade={shades.get(item.pane.id) ?? 0}
        onFocus={() => onFocusPane(item.pane.id)}
        onReady={onReady}
        onExit={onExit}
        retry={retrying.get(session.id)}
        onRetry={onRetry}
        onStopRetry={onStopRetry}
        onQuit={onQuit}
        onIntegration={onIntegration}
        onDirectory={onDirectory}
        onAttention={onAttention}
        onActions={onActions}
        onCopy={onCopy}
        onNotice={onNotice}
      />;
    })}

    {shown.map((item, index) => {
      const focused = layout.focusedPaneId === item.pane.id;
      return <section
        key={item.pane.id}
        className={`workspace-frame ${focused ? "focused" : ""} shade-${shades.get(item.pane.id) ?? 0}`}
        style={frameStyle(item.rect)}
        aria-label="Terminal workspace"
      >
        <div
          className="tabbar"
          data-tauri-drag-region={item.rect.top === 0 ? true : undefined}
          onPointerDown={() => onFocusPane(item.pane.id)}
        >
          {!sidebar && index === 0 && <button
            className="icon-button sidebar-open"
            title="Show sidebar (Ctrl Shift B)"
            onClick={onOpenSidebar}
          ><PanelLeftOpen size={16}/></button>}
          <div className="tabs">{item.pane.tabs.map(id => {
            const session = sessionsById.get(id);
            if (!session) return null;
            /* The pointer is captured by the tab, so the drag survives the
               cursor leaving the strip and the tab moving out from under it —
               the same shape the dividers use. Not while renaming, where the
               caret is what the pointer is for, and not from the close button,
               where a slipped press must still close the tab it was aimed at
               rather than shuffle the strip. */
            const waiting = attention.get(session.id);
            return <button
              key={session.id}
              className={`tab ${item.pane.activeId === session.id ? "active" : ""} ${waiting ? "attention" : ""}`}
              onClick={() => onActivateTab(item.pane.id, session.id)}
              onDoubleClick={() => onStartRename(session.id)}
              title={renaming === session.id ? undefined : "Drag to reorder; double-click to rename"}
              onPointerDown={event => {
                if (event.button !== 0 || renaming === session.id) return;
                if ((event.target as HTMLElement).closest(".tab-close")) return;
                event.currentTarget.setPointerCapture(event.pointerId);
                tabDrag.current = {
                  paneId: item.pane.id,
                  sessionId: session.id,
                  pointerId: event.pointerId
                };
              }}
              onPointerMove={reorderFromPointer}
              onPointerUp={event => {
                if (tabDrag.current?.pointerId === event.pointerId) tabDrag.current = null;
                if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                  event.currentTarget.releasePointerCapture(event.pointerId);
                }
              }}
              onPointerCancel={() => { tabDrag.current = null; }}
              onLostPointerCapture={() => { tabDrag.current = null; }}
            >
              {/* In place of the connection dot rather than beside it: a tab
                  asking for you is connected, and one mark is what the strip
                  has room for. An icon rather than a bigger dot, because a
                  change of shape reads where a change of colour does not —
                  across a full strip, and in a theme whose amber is grey. A
                  bubble when the program said something, which is an agent;
                  an empty one when it went quiet after working, which is
                  usually an agent that has not been told to say anything; a
                  bell when all it did was ring. */}
              {waiting
                ? <i className="waiting" title={waiting.message
                    ?? (waiting.quiet ? "Went quiet after working: it may be waiting for you" : "Waiting for you")}>
                    {waiting.message
                      ? <MessageSquareMore size={13}/>
                      : waiting.quiet ? <MessageSquare size={13}/> : <Bell size={13}/>}
                  </i>
                : <i className={`state ${session.state}`} title={session.state}/>}
              {renaming === session.id
                ? <input
                    className="tab-rename"
                    autoFocus
                    value={draftName}
                    onChange={event => onDraftName(event.target.value)}
                    onBlur={onCommitRename}
                    onClick={event => event.stopPropagation()}
                    onKeyDown={event => {
                      event.stopPropagation();
                      if (event.key === "Enter") onCommitRename();
                      else if (event.key === "Escape") onCancelRename();
                    }}
                  />
                : <span>{session.name}</span>}
              <span
                className="tab-close"
                role="button"
                onClick={event => { event.stopPropagation(); onCloseSession(session.id); }}
              ><X size={12}/></span>
            </button>;
          })}</div>
          <button
            className="new-tab"
            title="New session in this workspace (Ctrl Shift N)"
            onClick={() => onNewSession(item.pane.id)}
          ><CirclePlus size={16}/></button>
          {/* Says the other workspaces are still there, and how to get them
              back — without it a zoomed window reads as one whose splits
              were lost. */}
          {item.pane.id === zoomed && geometry.panes.length > shown.length && <button
            className="zoomed"
            title="Show every workspace again (Ctrl Shift Z)"
            onClick={onUnzoom}
          ><Minimize2 size={12}/> Zoomed · {geometry.panes.length - 1} more</button>}
        </div>
      </section>;
    })}

    {geometry.dividers.map(divider => <div
      key={divider.id}
      className={`workspace-divider ${divider.orientation}`}
      style={dividerStyle(divider)}
      role="separator"
      aria-orientation={divider.orientation === "columns" ? "vertical" : "horizontal"}
      tabIndex={0}
      title="Drag to resize; double-click to balance"
      onDoubleClick={() => onResizeSplit(divider.id, 0.5)}
      onKeyDown={event => {
        const backward = divider.orientation === "columns" ? "ArrowLeft" : "ArrowUp";
        const forward = divider.orientation === "columns" ? "ArrowRight" : "ArrowDown";
        if (event.key !== backward && event.key !== forward) return;
        event.preventDefault();
        const ratio = constrainRatio(
          divider.orientation,
          divider.rect,
          divider.ratio + (event.key === forward ? 0.05 : -0.05)
        );
        onResizeSplit(divider.id, ratio);
      }}
      onPointerDown={event => {
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = {
          id: divider.id,
          pointerId: event.pointerId,
          orientation: divider.orientation,
          rect: divider.rect
        };
      }}
      onPointerMove={resizeFromPointer}
      onPointerUp={event => {
        if (drag.current?.pointerId === event.pointerId) drag.current = null;
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
      }}
      onPointerCancel={() => { drag.current = null; }}
      onLostPointerCapture={() => { drag.current = null; }}
    />)}
  </div>;
}
