import React from "react";
import ReactDOM from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { failureOf, loadProfiles } from "./lib/backend";
import type { SessionProfile } from "./types";
import { applyTheme, storedTheme } from "./theme";

// The palette goes onto the document before React mounts. Every colour in the
// stylesheet is a custom property now, so a component that rendered ahead of
// this would paint against an empty one.
applyTheme(storedTheme());

// afterm used to reopen the tabs and the split it closed with, and no longer
// does: a launch is an empty window. The keys that carried it are dropped on
// the way past rather than left to sit in local storage — they hold the host
// and user of every tab that was open when the last version of afterm ran, and
// nothing reads them now.
for (const key of ["afterm.openSessions", "afterm.workspaceLayout", "afterm.restoreSessions"]) {
  localStorage.removeItem(key);
}

// The webview's default context menu offers Reload, Save as and Print, which
// belong to a web page rather than a terminal. Suppress it everywhere; text
// fields keep their own selection behaviour, and the terminal claims the right
// button first, for paste.
window.addEventListener("contextmenu", event => event.preventDefault());

const root = ReactDOM.createRoot(document.getElementById("root")!);

// Saved hosts decide the first screen, and whether the connect dialog opens by
// itself. Load them before mounting, or a returning user is asked where to go
// for the moment before their own hosts arrive.
//
// A store that cannot be read is said out loud rather than swallowed. It used
// to become an empty list, which is indistinguishable from having no saved
// hosts — and afterm would then write that emptiness back over the file at the
// next save. The backend leaves the file alone and says why; this is where the
// user hears it.
void loadProfiles()
  .then(profiles => ({ profiles, notice: "" }))
  .catch((problem: unknown) => ({
    profiles: [] as SessionProfile[],
    notice: `Your saved hosts could not be loaded. ${failureOf(problem).message}`
  }))
  .then(({ profiles, notice }) => root.render(
    <React.StrictMode>
      <ErrorBoundary><App initialProfiles={profiles} initialNotice={notice}/></ErrorBoundary>
    </React.StrictMode>
  ));
