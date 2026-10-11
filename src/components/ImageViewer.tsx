import { useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Download, X } from "lucide-react";
import { failureOf, previewRemoteFile } from "../lib/backend";
import { formatBytes } from "../lib/format";
import { imageType } from "../lib/preview";
import type { FileEntry } from "../types";

interface Props {
  sessionId: string;
  /** The images in the directory, in listing order, for ← and → to walk. */
  images: FileEntry[];
  start: number;
  onClose(): void;
  onDownload(entry: FileEntry): void;
}

type Shown =
  | { state: "loading" }
  | { state: "ready"; url: string }
  | { state: "failed"; message: string };

/**
 * An image from the host, looked at where it is.
 *
 * Fetched through the session's own transfer and held only as an object URL
 * for as long as the viewer is open — nothing lands in Downloads, and every
 * URL is revoked when the viewer closes. Shown fitted to the window; a click
 * shows it at its own size, scrolled. ← and → go to the other images in the
 * directory, Esc closes, and Download saves the one on screen the ordinary
 * way.
 */
export function ImageViewer({ sessionId, images, start, onClose, onDownload }: Props) {
  const [index, setIndex] = useState(start);
  const [shown, setShown] = useState<Shown>({ state: "loading" });
  const [actual, setActual] = useState(false);
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  /// Every fetch made while open, by path, as the promise of its URL: walking
  /// back is instant, walking back to one still arriving waits for that same
  /// fetch rather than starting another, and closing revokes every URL made.
  const fetched = useRef(new Map<string, Promise<string>>());
  const entry = images[index];

  useEffect(() => {
    const cache = fetched.current;
    return () => {
      for (const url of cache.values()) void url.then(made => URL.revokeObjectURL(made), () => {});
    };
  }, []);

  useEffect(() => {
    if (!entry) return;
    setActual(false);
    setSize(null);
    let request = fetched.current.get(entry.path);
    if (!request) {
      request = previewRemoteFile(sessionId, entry.path).then(bytes =>
        URL.createObjectURL(new Blob([bytes], { type: imageType(entry.name) ?? "application/octet-stream" })));
      fetched.current.set(entry.path, request);
      // A failure is not kept: the next visit asks again.
      request.catch(() => fetched.current.delete(entry.path));
    }
    // The image asked for may no longer be the one on screen when it arrives.
    let current = true;
    setShown({ state: "loading" });
    request.then(url => {
      if (current) setShown({ state: "ready", url });
    }, problem => {
      if (current) setShown({ state: "failed", message: failureOf(problem).message });
    });
    return () => { current = false; };
  }, [entry, sessionId]);

  // Ahead of the terminal and the app's own chords: arrows and Escape belong
  // to the viewer while it is open.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      if (event.key === "Escape") onClose();
      else if (event.key === "ArrowLeft") setIndex(at => Math.max(0, at - 1));
      else if (event.key === "ArrowRight") setIndex(at => Math.min(images.length - 1, at + 1));
      else return;
      event.preventDefault();
      event.stopPropagation();
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [images.length, onClose]);

  if (!entry) return null;
  const details = [
    size ? `${size.width} × ${size.height}` : null,
    entry.size !== null ? formatBytes(entry.size) : null,
    images.length > 1 ? `${index + 1} of ${images.length}` : null
  ].filter(Boolean).join(" · ");

  return <div
    className="image-viewer"
    role="dialog"
    aria-label={`Preview of ${entry.name}`}
    onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}
  >
    <header>
      <div>
        <strong title={entry.path}>{entry.name}</strong>
        <span>{details}</span>
      </div>
      <button type="button" title="Download" onClick={() => onDownload(entry)}><Download size={14}/></button>
      <button type="button" title="Close (Esc)" onClick={onClose}><X size={15}/></button>
    </header>
    <div
      className={`image-stage ${actual ? "actual" : ""}`}
      onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}
    >
      {shown.state === "loading" && <p className="image-note">Fetching {entry.name}…</p>}
      {shown.state === "failed" && <p className="image-note failed">{shown.message}</p>}
      {shown.state === "ready" && <img
        src={shown.url}
        alt={entry.name}
        title={actual ? "Click to fit the window" : "Click to see it at full size"}
        onClick={() => setActual(value => !value)}
        onLoad={event => setSize({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })}
        onError={() => setShown({ state: "failed", message: `${entry.name} is not an image this viewer can draw` })}
      />}
    </div>
    {index > 0 && <button type="button" className="image-step previous" title="Previous image (←)" onClick={() => setIndex(index - 1)}>
      <ChevronLeft size={22}/>
    </button>}
    {index < images.length - 1 && <button type="button" className="image-step next" title="Next image (→)" onClick={() => setIndex(index + 1)}>
      <ChevronRight size={22}/>
    </button>}
  </div>;
}
