# afterm changelog

What is built, and why it was built that way. `ROADMAP.md` holds what is not
built; `OPEN-ISSUE.md` holds the reviews and the decisions they turned on.

Entries were written as work landed rather than against release dates, so the
list below is everything that reached 1.0.0 rather than a diff between two
versions. Later releases get their own heading.

## Unreleased

- **A program on the host can copy to this machine's clipboard (OSC 52).**
  A long command from a coding agent arrives already wrapped by the agent,
  with real newlines and an indent on every continuation row, so a drag
  across it copies four or five broken lines. Windows Terminal and iTerm2
  copy it the same way, because the newlines really are there. Claude Code's
  `/copy` sends the exact text as OSC 52 over SSH instead, as tmux and vim do,
  and afterm now puts that on the clipboard with the usual "Copied" flash.
  It writes only: a query asking for the clipboard back is never answered, so
  no host can read what was copied here.

- **A tab is marked when its agent goes quiet, with nothing set up on the
  host.** An agent left as installed sends a terminal it does not recognise
  nothing, and over SSH none recognises afterm, so the mark needed a setting
  in every agent on every host. Now a tab whose output kept coming for two
  seconds and then stopped for three gets an empty amber bubble. Claude Code
  and Codex were measured first: their spinners redraw at least every 0.35 s
  and 0.5 s through a turn, and nothing at all after. Typing's echo does not
  count, and the watch is armed by typing into the tab or looking at it, then
  fires once, so a banner, a tmux status line or a bursty log marks nothing.
  A signal the agent does send still wins, and carries its message; a quiet
  mark never replaces one.

- **A waiting tab is marked by an icon, not a bigger dot.** An amber speech
  bubble when the program said something — an agent's notification, its
  message on the tooltip — and an amber bell when it only rang, both in place
  of the connection dot. The enlarged dot was easy to miss, and in a theme
  whose amber is grey it was only a slightly larger dot.

- **The file pane's toolbar fits the pane.** Parent, refresh and Download stay
  in the row; Upload files, Upload a folder and New folder moved behind a `⋯`
  at its end, which opens on hover or a click. The six buttons overflowed the
  pane at its default width.

- **Fixed: the image preview drew nothing.** The window's content security
  policy allowed images from the app, `asset:` and `data:` but not `blob:`,
  and the viewer shows a fetched image as an object URL — so every preview
  was refused and reported as not an image the viewer could draw.

- **Fixed: terminals were wider than their pane.** The fit addon sizes the
  terminal from its container's computed width and height, and under the
  global `border-box` those included the container's padding, so every
  terminal came out two or three columns and a row too large and its last
  columns ran into the edge — plainest in two workspaces side by side. The
  container is measured by its content box now, with the same 14px margin on
  both sides.

- **Paste a screenshot into an agent on the host.** With an image on the
  clipboard and no text, `Ctrl Shift V` or right-click in an SSH tab sends it
  to `~/.cache/afterm/paste/paste-<UTC time>-<4 hex>.png` on the host and
  pastes that path, which Claude Code attaches as `[Image #1]`. Text still
  wins when there is any. The image goes as the PNG the copying program left
  (Greenshot, the Snipping Tool, browsers); a bare bitmap from Print Screen is
  converted, 24- and 32-bit. The folder is made `0700`, pastes older than a
  week are cleared from it after each new one, and a corner line says so when
  the send takes long enough to notice or fails. Both transports; a local
  shell is left alone, since an agent there reads this clipboard itself.

- **Preview an image without downloading it.** Double-click an image in the
  file pane, or its eye icon, and it opens over the window: fitted, a click
  for full size, ← and → through the other images in the directory, Esc to
  close, and Download for the one worth keeping. It is fetched over the
  session's own transfer into a temporary file, read back and deleted at once,
  and held only as an object URL until the viewer closes, so nothing lands in
  Downloads. PNG, JPEG, GIF, WebP, AVIF, BMP, ICO and SVG, up to 64 MB — an SVG
  is drawn as an image, which runs none of its script. A double click on any
  other file still does nothing.

- **Open source, under the MIT license.** afterm is free software, and the
  source lives at [github.com/afarzin1/afterm](https://github.com/afarzin1/afterm).
  The license-key check, the evaluation reminder, *Settings ▸ License*, and the
  key-issuing worker (`license-worker/`, `scripts/license-issue.mjs`) are gone.
  Nothing was held back before, and there is now nothing to unlock: every
  build is the whole of afterm. A `license.txt` left beside the saved hosts by
  an earlier build is ignored.
  Windows builds and releases come from GitHub Actions
  (`.github/workflows/build.yml`), which replaces the Azure DevOps pipeline,
  and `scripts/get-afterm.ps1` updates from the latest GitHub release.

- **A shell that outlives its connection.** Off until *Settings ▸ Keep shells
  running in tmux* is switched on; with it off, none of its controls show and a
  host that has it switched on opens an ordinary shell, keeping the switch for
  when the feature is back on. Then per saved host — a switch on its row in
  the list, and a box on the new-host form — the terminal runs
  `tmux new-session -A -D` under a name that belongs to the tab, instead of a
  login shell. A dropped connection takes only the tmux client with it; the
  reconnect, by hand or on its own, attaches to the same session, so a build
  keeps running and the shell keeps its directory and variables. `-D` detaches
  the dead client the host has not noticed yet, which would otherwise hold the
  window to its old size. Closing the tab is what ends it, with a
  `kill-session` on the connection there is, or a batch-mode `ssh` that never
  asks for a password; a reconnect also closes the session on its way, and
  does not. tmux rather than anything of afterm's, because afterm installs
  nothing on a host; one without tmux gets its ordinary login shell and a line
  saying why. The cost is written next to the switch: the scrollback becomes
  tmux's, and inside tmux afterm cannot see commands or the shell's directory,
  so highlighting, jumping, find beyond the screen and the file pane following
  along stop, and an agent can signal only with the bell. Covered live against
  a real sshd and tmux.

- **Links open, on Ctrl click.** Printed URLs and OSC 8 hyperlinks both, and
  both went nowhere before: the link add-on's default is `window.open`, which
  the webview does not follow, and OSC 8 had no handler at all. Hovering shows
  where a link really goes before anything opens — an OSC 8 link is drawn on
  text of the host's choosing, and "open the dashboard" can point anywhere. The
  backend opens `http` and `https` with a host and nothing else (`links.rs`):
  `file:`, `search-ms:`, `ms-msdt:` and the rest of the Windows scheme handlers
  are how a click becomes code running on this machine. Ctrl, the way Windows
  Terminal and VS Code do it, because a plain click in a terminal is for
  selecting.

- **Jump between commands, and copy what one printed.** `Ctrl Shift PgUp` and
  `PgDn` scroll to the previous and next command, landing it on the second
  row so the pinned command does not cover it. `Ctrl Shift O` selects and
  copies a command's output: the one scrolled to, or with the view at the
  bottom, the most recent command that printed anything. The output runs to
  the next prompt the shell marked (OSC 133 `A`), the end it marked (`D`), or
  the next command; without shell integration the commands afterm banded from
  keystrokes serve, so it works either way. Also in the palette.
- **Quick select.** `Ctrl Shift S` labels every address, hash, container id,
  UUID, path, URL and long number on screen. Typing a label copies it, and
  Shift-typing it pastes it at the prompt too. The same text shown twice gets
  one label, one letter each until there are more than 26 and two letters each
  after, so no label is the start of another. Wrapped lines are read whole, so
  a long URL is one match. Anything that is not a label, or the screen
  scrolling, closes it.
- **Zoom a workspace.** `Ctrl Shift Z` fills the window with the focused
  workspace; a chip in its tab strip says how many are behind it and brings
  them back. The others stay mounted, so their terminals keep running and keep
  their scrollback. Moving focus, splitting and closing the zoomed workspace
  each end the zoom, by one rule rather than three special cases.

- **A tab marks itself when the coding agent in it is waiting for you.** An
  amber dot (an icon since; see above), carrying the agent's message as its tooltip, in place of the
  connection dot, until the tab is looked at; with the window behind something
  else the taskbar button flashes as well. Read from the signals the agents
  already send — OSC 9, OSC 777 `notify`, kitty's OSC 99 and the bell — rather
  than from recognising the agent, which afterm cannot do over SSH. A signal
  from the tab you are looking at marks nothing, which keeps a tab-completion
  bell quiet, and `OSC 9;4` progress reports are ignored so a background build
  does not light its tab. *Settings ▸ Coding agents* copies the setting Claude
  Code and Codex each need on the host, and a hook line for anything else.

- **A dropped session reconnects by itself.** Every five seconds for five
  minutes, counted from the drop. The popup shows a countdown and the last
  attempt's error, and has *Stop retrying* (`S`) beside Reconnect now and
  Close tab. The next attempt starts five seconds after the last one
  *finished*, so a slow timeout or an open password dialog never has a second
  attempt stacked on it. Only an unreachable host is retried; a refused
  credential, a changed host key or a cancelled dialog ends the retrying. A
  session has to stay up ten seconds to count as back, because an `ssh.exe`
  attempt looks connected before it has reached anything. Pressing Reconnect
  after the pane gives up starts the five minutes again.
- **A reconnect keeps what was on screen.** Each attempt builds a new
  terminal, which used to start blank; the old one's text now comes along,
  dimmed, so the output that explained the drop survives the first automatic
  attempt five seconds later.
- **The lost-connection popup is no longer drawn under a highlighted command.**
  xterm puts its decorations at `z-index` 6 and the terminal surface did not
  contain them, so the command band, which the popup's `z-index` 3 could not
  beat, showed straight through it. The surface is its own stacking context
  now. The find bar and the pinned command sat over the same bands and are
  fixed with it.
- **A failed connect says why in the terminal.** It printed
  `afterm: [object Object]`, because the backend's error is a structured value
  and was put through `String`; it prints the message now.

- **Deleting a folder deletes what is in it.** It used to go through a single
  `rmdir`, which the server refuses on anything that is not empty, and the
  refusal SFTP carries is the bare word "Failure", so a full folder could not
  be deleted from the pane and the pane could not say why. It now goes the way
  it does in WinSCP and `rm -r`. Files and links are removed as the tree is
  listed, then the directories deepest first. A link is removed as itself and
  never walked into, so what it points at survives. On a folder the confirm
  button reads *Delete all*. Anything that will not go stays where it is,
  along with its parent, and the status line names the first refusal.

- **A folder can be uploaded, by button or by dragging it in.** The other half
  of the folder transfer: the picker has a second button for a folder, because
  a system dialog offers files or directories and never both, and a folder
  dragged from Explorer onto the pane now goes rather than failing on the first
  attempt to read it. Which one a dropped path is has to be asked of the
  backend — a drop carries paths and nothing else, and the page has no
  filesystem to look at.

  It is the same walk as the download, read the other way round, with the same
  three rules: a link is never followed, nothing already on the host is written
  over without an answer, and one file that will not go costs that file rather
  than the tree.

  **The answer about a name already taken is asked once, about the folder.** A
  file's question — Replace, Keep both, Skip — reads differently over a folder,
  so it is asked differently: *Merge*, Keep both, Skip. Keeping both puts the
  folder under the next free name, exactly as a second download lands in
  `reports (1)`, and refusing sends nothing at all. Merging is the only answer
  under which a file on the host is written over, and it writes over the files
  inside that collide and leaves everything else alone.

  It is one question rather than one per file, which is the trade written down
  in `ROADMAP.md`: a folder of four hundred files must not be four hundred
  dialogs, and somebody who has said "merge" about a folder has said it about
  what is in it. Every file still goes through the single-file replacement with
  all of its care — staged under a private name, moved into place, wearing the
  permissions of what it replaced — so a directory in the way or a setuid bit
  is refused rather than replaced, and shows up in the count of what did not go.

  The question the host asks is what the pane asks on. `mkdir` is an exclusive
  create on both transports, and version 3 of the protocol reports a taken name
  as a plain failure with nothing in it to match on — so a refused `mkdir` is
  followed by a look at the name: something there means the name is taken,
  which is a question, and nothing there means the refusal was about something
  else, which is an error. Nothing is decided from a listing the pane happens
  to be holding.

  Proved where it can be: the in-process host can now make directories, and
  refuses a name anything is already at with the generic status a real server
  sends, so the fresh upload, the refusal, keeping both and merging are all
  `cargo test` — which means they also run on the Windows agent. The live
  OpenSSH test sends a tree to a throwaway `sshd` and gets refused when it
  sends the same one again.

- **A folder can be selected, and a selected folder can be downloaded.** A
  click in the file pane selects the row it lands on, whatever kind it is, and
  a directory opens on a double click or on Enter.

  It used to open on the single click, which meant a directory was the one
  thing in the pane that could never be *held*: every button in the toolbar
  acts on the selection, and a folder could not be one. So Download worked on
  files only — not because downloading a folder was refused, but because there
  was no gesture that would offer it one. WinSCP, MobaXterm, FileZilla and both
  desktop file managers have selected on a click and opened on a double click
  for thirty years; the pane was the odd one out, and the only thing the old
  behaviour saved was a click.

  Because the gesture moved, the first folder click of a run says where it
  went. Once per run, not once per click: that line is shared with transfers
  and failures, and a hint that repeats forever buries the messages that are
  not hints.

  **The download itself is new.** `download_folder` walks the tree over
  whichever transport the session already has — both of them can list a
  directory and fetch a file, and a folder is those two in a loop, so the walk
  is written once and neither transport can disagree with the other about what
  it does.

  Three rules shape it, and each one is a way a recursive download goes wrong.
  It never follows a link, because following one lets the copy leave the folder
  that was asked for and lets a loop run until the disk is full — OpenSSH's own
  `get -r` skips non-regular files for the same reason. Every remote name goes
  through `safe_name` and is then created exclusively, so a host serving a file
  called `../../evil` gets one called `.._.._evil`, and a folder already in
  Downloads is never written into: a second copy of `reports` lands in
  `reports (1)`. And one file that cannot be read costs that file rather than
  the tree — a failure about the path is counted and stepped over, a failure
  about the connection ends the walk, because every remaining file would fail
  the same way.

  What arrives is therefore reported rather than assumed: *Downloaded 12 files
  from reports to …Downloads/reports — 2 entries skipped, 1 could not be copied*.
  A status line that stopped at the path would be telling the user they have a
  complete copy of something they may not.

  A folder lands under its real name rather than travelling under a `.part`
  name and being moved into place, which is what a single file does. Renaming a
  *directory* onto an existing name is not portable — POSIX replaces an empty
  one and refuses a full one, Windows refuses both — and a commit step that
  behaved differently on the platform afterm ships on from the one it is
  developed on is worse than no commit step. The promise that matters survives
  either way: nothing already in Downloads is touched. The cost is in
  `ROADMAP.md` under "Known limits".

  Proved on both transports rather than on the one that was convenient: the
  in-process host now serves a real nested directory instead of answering every
  listing with the same four entries, and the live OpenSSH test walks a tree on
  a throwaway `sshd` — nesting, a name with a space in it, and a link that is
  stepped over.

- **A session that falls back to the OpenSSH client says why.** It used to
  discard the reason entirely — `Err(_)` — and the tab that appeared had never
  asked for anything and never explained itself.

  The case that matters is a host nobody has connected to before. Its key is
  not in `known_hosts`, so afterm's own connection refuses it *before* offering
  a credential, which is the right order and an invisible one: the fallback
  starts the OpenSSH client, which asks whether to trust the key in the
  terminal rather than in a dialog, and the only trace of any of it was a
  failure in the file pane. A password-only host looked like a tab that simply
  never asked for a password — and then asked on the second attempt, once the
  key had been recorded, which made it look intermittent rather than ordered.

  The notice names the reason and points at the terminal, because that is where
  the question waiting to be answered actually is. Asking about the key in
  afterm's own dialog is the real fix and is in `ROADMAP.md`; this is the part
  that should never have been silent.

- **The file pane follows the shell with nothing to install.** `cd` in the
  terminal and the listing moves with it on any Linux host, whether or not the
  shell integration snippet has ever been pasted anywhere.

  It followed OSC 7 before, which meant it followed nothing at all for anyone
  who had not found the snippet and put it in their `~/.bashrc` — which is
  most people, and the feature may as well not have existed for them. The
  headline behaviour of a file pane should not be behind a shell script the
  user has to install by hand.

  So it reads the directory off the host instead of waiting to be told. The
  telemetry stream already runs a loop on the connection reading `/proc` every
  two seconds; the shell's directory is two more reads on the same channel. The
  probe and the interactive shell are channels of one connection, so they are
  children of the same `sshd` and the probe can find its sibling with no PID
  passed in from outside: the sibling with a terminal is the shell, and the
  process group in the *foreground* of that terminal is what the user is
  actually typing at. That last part is what makes it survive nesting — reading
  the sibling directly answers with the outer shell's directory and goes stale
  the moment anyone types `bash` or `sudo -i`.

  It is the kernel's own child list rather than a sweep of `/proc`, because a
  sweep stats every process: 530ms a sample on a 195-process host, paid every
  two seconds for the life of the session. Two reads instead, about 2ms.

  OSC 7 still wins where the snippet is installed, because it arrives with the
  prompt rather than up to a poll later. The two disagree in one place — a
  nested `ssh`, where the announcement carries a path on the far machine and
  the read carries the client's own directory, which is the one this session
  can list — and the announcement still wins, because latency is what the user
  feels and the nested case is rare.

  Not everything: it needs `/proc`, so an old Unix host gets nothing, which is
  the same limit MobaXterm documents for the same feature. A shell afterm may
  not look at — `sudo -i` leaves a root-owned process — reports nothing rather
  than an error, and the pane stays where the user had it.

- **The shell snippet can be installed by a button.** *Settings ▸ Shell
  integration ▸ Add to this host* appends it to `~/.bashrc` or `~/.zshrc` over
  the session's own connection, idempotently, and reports which file it chose.

  Pasting a shell script into a startup file by hand was a bad ask, and the
  reason it was only ever offered as a paste — that afterm should not edit a
  file on a machine that is not its to edit — was not a rule the app actually
  held: *install key* has always written to `~/.ssh/authorized_keys`. The
  distinction worth keeping is not *copy versus write*, it is *asked for versus
  automatic*. This is asked for, once, by someone who wants it, and it reports
  back everything it did.

  It answers the failure nobody would otherwise diagnose: an SSH session gets a
  *login* shell, which reads `~/.bash_profile` and not `~/.bashrc`. It works on
  most distributions only because their profile sources the rc file, and where
  it does not the snippet is installed and will never run. The host is asked,
  and says so.

  A login shell that is neither bash nor zsh is refused by name rather than
  guessed at, and *Copy snippet* stays for a host you would rather paste into
  yourself.

- **Copy on select is on by default, and says so.** Highlight with the mouse
  and the text is on the clipboard when the button comes up, without anyone
  having to find the setting first.

  What kept it off was that the copy was silent. Every drag spends the
  clipboard, so highlighting a line to read it threw away the address that was
  waiting there to be pasted, and the user found out at the paste — which is
  too late and a long way from the cause. That is a bad surprise whatever the
  feature is worth, and it is the whole of the case for leaving it off.

  So the drag answers for itself: a write that lands puts *Copied to
  clipboard* in the bottom corner, holds it long enough to read out of the
  corner of an eye, and fades it. The overwrite stops being silent, the
  surprise goes with it, and the default can be what the people arriving from
  PuTTY and from X11 already expect. Anyone who wants it back the old way
  turns it off in *Settings* and that answer is kept — a machine that has
  never been asked is not the same as one that said no.

  The message is for copy-on-select only. `Ctrl Shift C` is a key pressed on
  purpose and needs no receipt. And it waits on the clipboard write
  succeeding, because announcing a copy that the clipboard refused would send
  someone off to paste something that is not there.

- **The file pane follows the shell.** `cd` in the terminal and the listing
  moves with it, on the same snippet that already marks commands: OSC 7 carries
  the directory, the shell sends it at every prompt, and the pane goes there.

  It moves once per change, and that is the whole design rather than a detail
  of it. A shell reports its directory at every prompt, so following the
  *report* would drag the pane back the instant anyone browsed anywhere else —
  the pane would be unusable for its main job, which is looking at a directory
  that is not the one the shell is sitting in. What the pane follows is the
  change: the last directory it acted on is remembered per session, and a
  report that matches it does nothing at all.

  Switching to another session and back does not cost the place either. Each
  session's pane remembers where it is, so a switch restores what was on
  screen rather than reopening at the home directory, and a shell that moved
  while the pane was looking elsewhere is followed when the pane comes back —
  once, because the change happened once.

  A followed move is quiet when it fails. The directory a shell reports is not
  always one this pane can list: a nested `ssh`, a `docker exec` or a `sudo`
  into another filesystem all report a path belonging to somewhere else, and
  the host half of OSC 7 cannot be checked against anything afterm knows, since
  the session was opened against whatever the user typed while the shell sends
  whatever `hostname` says. So a move nobody asked for that cannot be listed
  leaves the listing that is already there, instead of replacing it with an
  error about a path the user never navigated to.

- **Tabs reorder by dragging them.** Grab a tab and it travels along its
  strip with the pointer, the others parting around it; let go and it stays.
  The rule is in `dropIndex`, which counts how many of the *other* tabs'
  midpoints the pointer has passed. Leaving the dragged tab's own midpoint out
  of that count is what makes it work: counting it as well scores two for one
  neighbour crossed — its own, which it is still sitting on, and the
  neighbour's — so a drag one place to the right moved the tab two, and the tab
  ran ahead of the pointer.

  The strip is re-measured on every pointer move rather than read once on the
  grab, because the tabs move as the drag goes. That also supplies the dead
  zone: nothing happens until the pointer has travelled half a tab, which is a
  distance that means something instead of a threshold in pixels someone had to
  guess at. `moveWorkspaceTab` hands the same layout back when the tab is
  already where it would land, so most of those moves are a render React does
  not do.

  The pointer is captured by the tab, the way the split dividers already do it,
  so a drag survives the cursor leaving the strip. A press that lands on the
  close button never starts one, or a slipped click would shuffle the strip
  instead of closing the tab it was aimed at, and a tab being renamed is left
  to its caret. Dragging onto another workspace does nothing: moving a session
  between panes is a different operation, and not one a stray drag should be
  able to perform by accident.

- **PowerShell and Command Prompt are back, as second-class citizens.** Two
  buttons at the far end of the new-session dialog's tab strip. They look like
  the two tabs beside them and behave like the buttons they are: pressing one
  opens the shell and the dialog closes, because there is no second screen
  behind either. There is nothing to configure, nothing to save and nothing to
  name.

  The point of that shape is what it does *not* cost. The local shells were
  dropped for being an equal third of the dialog, and the bill was never the
  shells themselves: it was `isRemote` in the status bar, the tab icon, the
  file pane and the telemetry command, and an address that had to be optional
  on every profile because one kind of session had none. So the address is
  still required and is simply empty for a local shell; one predicate,
  `isLocalShell`, answers the question; and the screens that are about a far
  end say nothing rather than something invented. The status bar goes blank —
  no host, no dot, no readings, and nothing polled — because everything in it
  is about a machine that is not this one. The file pane says in a sentence
  that there is nothing here to browse from a distance. Installing a public key
  is disabled. In Rust the session's remote half is an `Option` again, so
  `session_metrics`, the file commands and the key installer all refuse in one
  place instead of each asking whether this session has a far end. And because
  nothing about one is saved, there is nothing to migrate if they go again.

- **An upload is private for the whole of its transfer, on the OpenSSH-client
  transport too.** `put` creates the staging file with the *local* file's mode
  and the batch language cannot pass attributes to a create, so the `chmod`
  that followed the transfer was the earliest narrowing there was — and
  everything before it was exposure. Replacing a `0600` remote file from a
  `0644` local one left the staging file world-readable for the whole transfer,
  with 40 KB of it readable at the point the reproduction stopped to look. The
  unguessable name was not the defence anyone hoped: a name is discoverable by
  listing, and by the time there is a file to find, there is content in it.

  The staging file now goes inside a directory of its own: `mkdir`, `chmod
  700`, then `put`, in that order and in the same batch. A directory nobody may
  enter has nothing to list. The one window left holds an empty directory under
  a 128-bit name, between the `mkdir` and the `chmod`, and there is no way to
  close it from the batch language — but nothing is in it. It also buys an
  exclusive create, which the single staging file never had: `mkdir` refuses an
  occupied name and `put` does not. Two tests: one reads the ordering off the
  batch itself with a stub client, because the end state was already correct
  when the bug was live; the other watches a 64 MiB replacement from outside
  and requires that nothing afterm staged is ever a file in the destination
  directory or a directory holding content at anything but `0700`.

- **The native terminal reads and writes on separate tasks.** One task did
  both, awaiting `channel.data_bytes` in the same place it drained the channel,
  and the two deadlock against each other on any host that answers while it is
  being written to: the write parks waiting for send window, the channel's
  receive queue fills because nothing is draining it, the connection's event
  loop blocks pushing into that queue, and the window adjustment the write
  needs is behind the block. A 12 MiB paste at an echoing host stopped at
  3,178,496 bytes with a 20-byte banner on screen and never moved again.

  Reading and writing never wait on each other now. Queued input is bounded —
  one write of any size is taken, because a paste is one thing somebody did,
  and a *backlog* past 8 MiB is refused and says so — and closing a tab
  overtakes a blocked write rather than queueing behind it, which is why the
  close signal is not a message on the queue the stuck write is at the front
  of. The test host had a deadlock of its own in the same place, holding a
  russh `Channel` nobody read; it is dropped now, so a regression in afterm
  cannot hide behind it.

  The in-process host answers a large write with one byte per chunk rather than
  echoing it, and that is a concession to the fixture rather than a weakening
  of the test. Both ends of it are russh session loops, and a russh session
  loop writing a large payload is not reading while it does: saturate both
  directions and they wedge against each other at the socket rather than at the
  SSH window. Linux's loopback buffers are large enough to hide that and
  Windows', a fraction of the size, are not — build 246 stopped at 393,216
  bytes sent and stayed there for two minutes. A real sshd reads while it
  writes. What the test needs from the far end is a count of replies, not a
  weight of bytes: the queue that filled holds a hundred messages, and hundreds
  of one-byte answers fill it just as well while fitting in any socket buffer
  there is. Against the single-task version it still stops dead, at 4,259,840
  bytes with twenty answers back.

  Overtaking the write turned out not to be enough, and the Windows agent is
  what said so: build 245 failed the close test while every other one passed.
  A host that has stopped reading its socket blocks the connection's own event
  loop, so the courtesy `close` afterm sends on the way out blocks too — just
  handing it over blocks — and the reader was waiting behind that before it
  reported the end. Linux's loopback buffers are large enough to swallow the
  whole write and hide it. The reader is released first now, whatever stopped
  the writer, and the close it does not need to wait for is given a deadline
  of its own rather than parking a task forever.

- **A session is not called connected until the host has said so.**
  `request_pty` and `request_shell` only submit their requests in the pinned
  russh; the acknowledgments arrive later as channel messages, and the receive
  loop ignored the failure variant. A host that refused the shell outright
  produced a green status light over a terminal that would never show anything.
  Both answers are awaited now, with a thirty-second patience and the channel
  taken down on a refusal, and anything the host says while it is still
  answering is kept and written to the terminal rather than read off the
  channel during setup and thrown away.

- **One unreadable saved host no longer costs you the rest.** `load_profiles`
  deserialized the whole array and fell back to an empty list, so a single
  entry this version could not decode — a `powershell` profile from before the
  local shells were dropped was the reproduction — emptied the dialog, and the
  next save wrote that emptiness over the file. Entries are decoded one at a
  time now. An entry afterm cannot show is carried through the next save
  verbatim rather than dropped, because an entry it cannot read is not one it
  gets to delete. A store that cannot be read or understood at all is reported
  to the user and left exactly as it is, rather than answered with "you have no
  saved hosts" — the answer that invites the next save to make it true; a save
  over one moves it aside to `profiles.json.unreadable` first, so afterm stays
  usable and the original is still there to look at.

- **A saved host can remember its password, in the operating system's own
  credential store.** Password-only hosts are what the native transport exists
  for, and retyping the password for every tab was the cost of it. Now the
  password dialog offers *Remember this password on this device* — on a saved
  host, for the password method, where the machine has a credential store — and
  the password is written after the host has accepted it, read back inside Rust
  into the handshake that needs it, and never returned to the interface. No
  command hands a saved password to JavaScript, `profiles.json` gains no field,
  and the profile's own id is what the entry is keyed on, so renaming a host
  keeps its password and deleting one removes it.

  The entry is bound to more than the profile. The stored blob carries the
  account, the resolved hostname, the port and the host key fingerprint beside
  the password, and all four are checked before it is used: a saved host
  repointed at another machine, or one that comes back with a different key, is
  asked about rather than handed the old password, and its entry is left alone
  because it is still right for wherever it was saved for. Keeping the binding
  inside the entry rather than in `profiles.json` is what makes checking it
  worth anything.

  Tried once per attempt, and the "once" is the part with teeth. A refused
  password counts against the host's own attempt limit, so a stale entry must
  not be walked past the server again by this tab retrying or by the other tabs
  on the same saved host; the refusal is recorded for the process, and the
  dialog that follows says the host refused the saved password rather than
  looking like afterm lost it. The attempt happens *before* the check for
  whether a dialog may be raised at all — using a remembered password asks
  nobody anything, so a telemetry poll and a click on the file pane now connect
  on one where either used to give up and wait for a human. That ordering was
  wrong in the first draft and a test caught it.

  Where there is no operating-system store there is no feature: the settings
  panel says which, the checkbox is not offered, and there is no encrypted-file
  fallback, because a file with its key in the executable is an obfuscated
  credential store and not a credential store. Windows Credential Manager is
  the store today, through `keyring-core` and `windows-native-keyring-store`,
  written `CRED_PERSIST_LOCAL_MACHINE` so the credential stays on this machine
  for this Windows user rather than roaming — the store crate's own default is
  `Enterprise`, which roams, so the persistence is passed on every entry rather
  than left to it. macOS is one dependency and one `Store` in `secrets.rs` when
  it comes; nothing above that module names a platform, and the interface asks
  whether saving is available and why not rather than which operating system
  this is.

  Settings says how many hosts have one and offers *Forget all*; each saved
  host's row offers to forget just its own. A store that refuses a write leaves
  the session connected and says the password was not saved, which is the one
  outcome that must never pass in silence.
- **The installer artifact holds only what the run built.** Build 241 published
  four installers — `afterm_0.1.0` and `afterm_1.0.0` of both the MSI and the
  NSIS setup — a day after 1.0.0 landed, and nothing in that run built 0.1.0.
  WiX and NSIS write into `target\release\bundle`, which is inside the
  cargo-target cache and which neither of them cleans, so the bump is what
  exposed it: editing Cargo.toml changed `manifestHash` and the exact cache
  key, the `restoreKeys` prefix fallback restored a target directory from
  before the bump, and the 0.1.0 installers sitting in it were published beside
  the ones the run had just built. The run then saved both under the new key,
  which is what made a one-off into a fixture — every later run restored it and
  published it again. The bundle directory is now deleted before packaging.
  Nothing is lost: WiX and NSIS repackage from the linked executable every run
  and never reused what was there, so the only thing the cached copy ever did
  was mislead whoever opened the artifact.
- **Copy on select, for the people who came from PuTTY.** Highlighting text
  with the mouse puts it on the clipboard when the button comes up, without
  `Ctrl Shift C`. Off until it is turned on under *Settings → Terminal*, and
  off is the honest default: turning it on means every drag spends the
  clipboard, so highlighting a line to read it loses the address that was
  waiting there to be pasted. Worth it for the people who expect it, a bad
  surprise for everyone else, which is what makes it a setting. Kept in
  `afterm.copyOnSelect`, and reachable from the command palette like every
  other setting. The copy hangs off the end of the drag — left button down on
  a pane, then the release — and not off xterm's `onSelectionChange`, which is
  the obvious hook and wrong twice: it fires for every cell the mouse crosses,
  a hundred clipboard writes for one drag, and it fires for selections the
  mouse never made, so walking through `Ctrl Shift F` results would have left
  the last match on the clipboard in place of whatever the user put there. The
  release is heard on the window because a drag that begins in a terminal can
  end over another pane, over the sidebar or off the edge of the window and is
  an ordinary drag every time; the button-down flag is what keeps a click
  somewhere else in the interface from copying a selection a pane is still
  holding. The right button stays out of it, or every right-click paste would
  answer itself by copying the selection back over the clipboard it had just
  read. The rule is three conditions in `lib/selection.ts` rather than an
  `if` in the pane, because each one is a way the feature goes wrong and each
  one now has a test.
- **A launch is an empty window.** afterm used to reopen the tabs and the split
  it closed with. It no longer does, and the reason is what a tab is here: not
  a document but a connection. Reopening one is not restoring a view, it is
  dialling a host — so a launch fired off however many handshakes the last run
  happened to end with, and asked for however many passwords those wanted,
  before the window had been looked at. Closing afterm is how a session ends,
  and the window that comes back is blank with the connect dialog on it. The
  `afterm.openSessions`, `afterm.workspaceLayout` and `afterm.restoreSessions`
  keys are removed at startup rather than left in local storage, because they
  hold the host and user of every tab an earlier version had open; the layout
  store and its reconciliation went with them. The theme, the terminal font
  size and the saved hosts are still remembered — a scheme, a size, and a list
  written on purpose.
- **A third ground, so no two panes that touch ever match.** Panes are told
  apart by shade rather than by an outline, and the shade was the parity of a
  pane's column plus its row — a checkerboard, which has only two colours to
  hand out. Two is not enough, and the layout that proves it is a common one: a
  pane beside a split touches both halves, those halves must differ from each
  other, and so one of them was forced to match the pane beside them. An odd
  cycle is not two-colourable. Each theme now carries a third ground,
  `--bg-alt-2`, one more step down the same ramp, and the shades are a graph
  colouring over the pane rectangles — reading order, lowest shade no
  already-shaded neighbour has. Touching at a corner does not count, so a grid
  still falls out as the checkerboard it always was, and the top-left pane is
  still shade 0, which is what keeps the pane against the sidebar contrasting
  with it. Four mutually adjacent rectangles would beat three shades and greedy
  colouring is not optimal even where three would do; neither is reachable by
  splitting the way afterm splits, so a pane with every shade taken around it
  falls back to the one fewest of its neighbours are using rather than paying
  for an exact search. Stockholm's ANSI black moved one step down with the
  ramp: the new third ground landed on `#12161e`, the value black used to hold,
  and black is the slot programs reach for as a background.

### The September 2026 review, fixed

Ten findings from two passes over the 1.0.0 build. The reproductions used
throwaway fixtures — an in-process SSH host, a local `sftp-server`, a local
OpenSSH client — and no production server was touched.

**Two the review called P1.**

- **A replacement no longer loses the permissions of what it replaced.** Native
  uploads created their staging file with no permissions of their own, so it
  took whatever the server's umask gave it and the rename carried that onto the
  destination: with `umask 022`, a private `0600` file came back `0644`, and an
  executable lost its bit. The staging file is now created `0600` in the `open`
  request itself — there is no moment at which the content exists under a wider
  mode — and the destination's own mode is applied to it *before* the move, so
  the file never appears under its real name wearing the wrong one. A mode that
  cannot be set abandons the upload rather than installing the file anyway. The
  OpenSSH-client transport gets the same rule, as far as its batch language
  allows: `put` sends the local file's mode at create time and cannot be given
  attributes, so the staging file is narrowed to `0600` the instant the
  transfer ends, under a name nobody can guess. The rules themselves —
  `fsmodel::carried` and `local_mode` — are shared, so both transports reach
  the same answer. Setuid, setgid and the sticky bit are refused rather than
  re-applied: they are a grant of privilege attached to particular content, and
  carrying one onto content from elsewhere is how a setuid binary becomes
  somebody else's. Ownership and ACLs are named as outside what SFTP can
  promise.
- **The configured cryptographic policy is honoured.** `Ciphers`, `MACs`,
  `KexAlgorithms`, `HostKeyAlgorithms` and `PubkeyAcceptedAlgorithms` were read
  and deliberately ignored, on the reasoning that russh negotiates from a
  modern set anyway. That is true of what afterm would choose and beside the
  point of what the user forbade. The effective lists from `ssh -G` are now
  intersected with what russh can negotiate, in the configuration's own order,
  and handed to russh as the client config — which is also what a rekey
  negotiates from, so the policy holds for the life of the connection.
  `HostKeyAlgorithms` is intersected with the algorithms `known_hosts` actually
  recorded, and `PubkeyAcceptedAlgorithms` gates which keys are offered and
  which RSA signature is used. An empty intersection is a refusal naming the
  directive, which sends the host to the OpenSSH client; it is never widened
  back to a default. The extension markers russh needs for `ext-info-c` are
  appended after the intersection, or narrowing `KexAlgorithms` would silently
  cost the server-sig-algs extension that tells afterm which RSA signature a
  host accepts.

**Three the review called P2, on the backend.**

- **Telemetry and diagnostics are bounded properly.** `Blocks::feed` charged a
  block for the length of its lines, so a stream of nothing but newlines was
  accounted at zero bytes while every empty row was retained — 100,000 of them
  accepted, at least 3 MB of row storage, and no limit reached. The newline is
  now charged with the line, so an empty row is never free, and a row-count
  limit stands beside the byte limit. Every capture of a helper's output —
  telemetry stderr, `sftp` stdout and stderr — goes through one `read_capped`
  that keeps a bounded prefix *and goes on draining*, because a pipe nobody
  empties is a child process that cannot exit. A truncated listing is reported
  as an error rather than shown as a directory with fewer files in it.
- **Public keys are installed through the connection, not the terminal.**
  `install_public_key` wrote a command into the active terminal, which was
  wrong three ways: after `sudo -i` or a nested `ssh` it ran as that shell's
  account on that shell's host while the dialog still named the session's; an
  editor in front of the terminal received it as keystrokes; and it ended with
  an unconditional `printf` announcing success, so a refused `mkdir` or a
  read-only home reported "public key installed" too. It now runs on a channel
  of afterm's own authenticated connection through `Connection::run`, with
  `set -e` so the exit status means something, and the account and path come
  back from the host and are checked before anything is claimed. On a session
  carried by the OpenSSH client the feature reports itself unavailable rather
  than being half-provided.
- **Collision handling is atomic where the transport can make it so.** The
  OpenSSH-client transport checked whether a name was taken and then committed
  with the client's own `rename`, which is `posix-rename@openssh.com` and
  replaces — so a file that appeared in between was overwritten by an upload
  that had been told not to overwrite anything. `Fail`, `Rename` and the file
  pane's own rename now commit with a hard link, which the server performs with
  `link(2)`: it refuses an existing destination atomically, and dropping the
  staging name turns it into a move. A server that cannot offer the extension
  is told about rather than quietly downgraded. `Replace` keeps the replacing
  rename, which is what it asked for. On the native transport `Fail` and
  `Rename` were already atomic — the protocol's own `rename` refuses a taken
  target — and that is now stated rather than left to be re-derived; `Replace`
  cannot be, because russh-sftp 2.4 exposes no extended request to send
  `posix-rename` with, so what is guaranteed instead is that the original is
  never destroyed and never anonymous: it is put back if the replacement will
  not go in, and if it cannot be put back the failure says where it is.

**Five the review called P2, in the terminal.**

- **Pasted and recalled commands are highlighted.** The input handler ignored
  everything beginning with Escape, which is both bracketed paste and the
  history keys — so neither started an input run and Enter had nothing to
  highlight or pin, the two cases people repeat most. Input is now read by
  `src/lib/commands.ts`, which distinguishes a paste (and does not treat a
  newline inside one as a submission), the history keys in both cursor modes,
  reverse search, line editing, and noise that is not input at all.
- **The band follows a command onto the rows it wrapped onto**, and waits for
  the echo before it is drawn. Reading the cursor at the moment Enter was
  pressed got a wrapped command wrong whenever the echo had not caught up,
  which over SSH is most of the time; the rows are now found through xterm's
  own wrap flag, on the first output after the Enter.
- **Highlighting recovers when shell integration disappears.** One OSC 133 mark
  used to switch the keystroke fallback off for the life of the tab, so
  `sudo -i` or a nested `ssh` into an unintegrated shell lost highlighting
  altogether — including for plainly typed commands. The question is now asked
  of each prompt. A half-integrated shell that sends `A` and `B` but never `C`
  is banded from the fallback after a grace period rather than never, and a row
  cannot be banded twice, so a late `C` and the fallback cannot both draw.
  Settings reports the shell in front of the terminal *now*, and a reconnect
  starts over.
- **Nothing a session emits can arrive before something is listening.**
  `listen` is an IPC round trip and the session was started before it finished,
  so early banners, the first prompt and the integration marks in it could be
  written into nothing — and `AuthDialog` renders after the grid, so a password
  request could be raised before its listener existed. Every backend stream is
  now registered as `backend.ts` is imported, `spawnSession` waits for all of
  them, and anything that still arrives before the first subscriber is held
  rather than dropped. A pane that closes while its subscription is in flight
  disposes it and does not spawn.
- **Command and highlight bookkeeping is bounded by the scrollback.** Band
  records were pruned only on a theme change and command markers never, so a
  long session accumulated dead records that every render and every scroll
  walked. Records are now removed when their marker is disposed, decorations
  with them, the pin is dropped when the command it points at is trimmed, and
  the pin's search is a binary search rather than a walk.

## 1.0.0 — 2026-09-06

### The native SSH transport

One authenticated `russh` connection per session, carrying the terminal, SFTP
and telemetry as channels on it. This is what MobaXterm, WinSCP and Termius do,
and it is why they work with password-only hosts: it removes the OpenSSH-client
transport's authentication gap rather than working around it.

It shipped in five stages, each provable before the next depended on it,
because afterm cannot be run on Windows locally and every regression in this
component had hidden there.

- **Stage 0 — honest failures.** Typed causes for both helpers
  (`transport::Failure`), a real lifecycle on the telemetry helper, the dead
  handle cleared on exit and retried. Before it, the telemetry helper reported
  success as soon as its process spawned and discarded the process's stderr, so
  a password-only host showed `no data yet` forever rather than `needs key
  auth`, and the dead helper was never retried — the message the README
  described could not be reached.
- **Stage 1 — the seam.** `Transport` with `Cli` and `Native` variants over one
  method set; remote commands become `async`; the terminal's `portable-pty`
  fields move behind a `Terminal` enum. No behaviour change.
- **Stage 2 — connection, trust, authentication.** One connection per session,
  channels on demand, one shared in-flight attempt so concurrent pollers cannot
  open two. `ssh-keygen -F` trust with `Preferred.key` pinned to the recorded
  algorithms; `ssh -G` configuration with named refusals; publickey, agent,
  password and keyboard-interactive, chosen by `remaining_methods`.
- **Stage 3 — files and telemetry on it by default.** The OpenSSH-client
  transport becomes the fallback, surfaced per host. The password-only gap
  closes, and so do the login per click, the audit noise and the missing
  transfer progress.
- **Stage 4 — the terminal, and the default.** The PTY channel shipped as its
  own session kind so both terminals could be open at once and compared. What
  the comparison showed was that a split transport default made a password host
  ask twice, once for the terminal and once for everything else, which is the
  friction the transport existed to remove. So the default flipped, and the
  tick survives inverted as `Run the terminal through the OpenSSH client` for
  anyone who wants the classic behaviour back.

Concretely, what that adds up to: password, keyboard-interactive, publickey and
agent authentication, chosen by what `authenticate_none` says the host will
accept, with keyboard-interactive a real challenge loop so a PAM or 2FA host
asking three different questions gets three different answers; host trust
through `ssh-keygen -F` with host key algorithms pinned to what is recorded;
configuration through `ssh -G`; a named refusal — falling back to the OpenSSH
client — for `ProxyJump`, `ProxyCommand`, `HostKeyAlias`, an SSH certificate
authority, or a `known_hosts` marker afterm does not understand; typed failures
end to end, so the file pane offers a password prompt for one cause and a retry
for another and the status bar says which transport is carrying the session;
and transfer progress.

**Confirmed on Windows, 2026-09-04.** Against a password-only Ubuntu server
from a Windows build: the native transport authenticates with a password, the
status bar offers the credential rather than going quiet, and files and
telemetry both come up on the connection. Opened as a native session instead,
one password covers the terminal as well. That was the gap in `OPEN-ISSUE.md`,
closed on the platform it mattered on.

Native SFTP is a structured protocol, so the quote-and-newline injection guards
the OpenSSH-client transport needs for `sftp` batch mode stop being necessary
on this path. Fewer sharp edges, not more.

### What the spike established

A spike was run against `russh` 0.63.2 and `russh-sftp` 2.4.0 (September 2026).
The code is preserved in `spikes/native-ssh/`. Findings, so none of this needs
re-deriving:

### What the spike established

A spike was run against `russh` 0.63.2 and `russh-sftp` 2.4.0 (September 2026).
The code is preserved in `spikes/native-ssh/`. Findings, so none of this needs
re-deriving:

**Everything afterm needs works on one connection.** Verified against a real
OpenSSH server: publickey auth, an exec channel returning real `/proc` data, an
SFTP subsystem, and a PTY channel running a shell — all on a single connection.

**Password authentication works**, along with keyboard-interactive, the shape a
PAM or 2FA host presents. Verified against an in-process russh server, because a
non-root `sshd` cannot verify passwords.

**`authenticate_none` reports the server's accepted methods.** That is how the
UI decides whether to show a password box, a key picker, or a 2FA prompt: ask
the server first rather than guessing.

**russh implements `known_hosts` itself** — `check_known_hosts_path`,
`learn_known_hosts`, and `Error::KeyChanged { line }` distinct from unknown. The
security-critical parsing does not have to be hand-rolled, which was the largest
worry going in.

**`check_server_key` defaults to `Ok(false)`.** It rejects every host key, and
the trait's own documentation says so. The spike originally recorded the
opposite. Overriding it is still mandatory and the no-credential-on-refusal test
still earns its place, but the failure mode of forgetting is a client that
connects to nothing, not one that trusts everything.

**Going native *removes* Windows-specific risk for SSH.** The PTY is negotiated
over the channel on the remote host, so there is no local process, no ConPTY,
and no platform-specific code path. `portable-pty` is still there, but only for
the OpenSSH-client transport — the local shells that were the other reason for
it have since been dropped.

**The whole security battery runs with no external server.** `russh-sftp` has a
server side, so an SSH server, an SFTP subsystem and the client all run inside
one test process on loopback. That is a plain `cargo test` — it runs on the
hosted Windows agent, which has neither an SSH server nor `sftp-server`. This
matters: Windows is exactly where a Windows-only regression would otherwise
hide.

**russh is pre-1.0 and the API moves.** Seven compile errors between a
from-memory implementation and 0.63.2: `PublicKeyOrCertificate::Certificate` is
a tuple variant, `MethodSet` is a `Vec` newtype rather than bitflags,
`channel_open_session` hands the server a handle that must be `.accept()`ed,
`Response` yields `Bytes`, `Session::data` takes `Bytes`, `rand::rng()` replaced
`OsRng`. All mechanical and all caught by the compiler in minutes — but read the
crate, do not trust recall.

### Test battery

In `src-tauri/src/native/battery.rs`. An SSH server, an SFTP subsystem and
afterm's client all run inside one test process on loopback, which makes the
whole thing a plain `cargo test` — and therefore something that runs on the
hosted Windows agent, a machine with neither `sshd` nor `sftp-server`.

In `src-tauri/src/native/battery.rs`. An SSH server, an SFTP subsystem and
afterm's client all run inside one test process on loopback, which makes the
whole thing a plain `cargo test` — and therefore something that runs on the
hosted Windows agent, a machine with neither `sshd` nor `sftp-server`.

*Host keys* — matching accepted; changed refused; unknown refused; a key of an
algorithm that is not recorded refused; and on every refusal, **no credential
offered**: an instrumented server counts credential checks and must see zero.

*Auth* — correct and wrong password; `remaining_methods` reports what the host
accepts, from the protocol rather than from parsed client output.

*Files* — a listing with directories first, sizes, a symlink, a name with
spaces, and a timestamp formatted the way the OpenSSH client formats it; a
missing directory is an error and not an empty listing; three concurrent
listings do not interleave, which was a real bug in the v1 subprocess
implementation and so earns a permanent test.

*Telemetry* — an exec channel on the same connection, parsed by the same
`/proc` parser the OpenSSH helper uses, with two samples so a rate can be
checked.

*Terminal* — a PTY channel carries output, carries typed input to the remote
end, sends `window_change` on resize with the right dimensions, and tears down
when closed.

*Elsewhere in the tree* — `ssh -G` parsing including the refusals; `ssh-keygen
-F` parsing including the `@revoked` and `@cert-authority` markers; OpenSSH
stderr classification; and that a saved profile structurally cannot hold a
credential.

*Against a real server* — `scripts/live-native-test.sh` stands up a throwaway
sshd and a throwaway `ssh_config`, then drives the whole path against it:
`ssh -G` resolving an alias, `ssh-keygen -F` finding the recorded key, the
handshake, SFTP, a streamed transfer, and telemetry from a real shell reading
real `/proc`. The in-process battery cannot reach those seams; this is where a
change to them gets caught. Ignored by default, `npm run test:native`.

*Against the real binaries* — these earned their place on the first Windows
run: `ssh -G` under Win32-OpenSSH reports its system paths with a literal
`__PROGRAMDATA__` token, which afterm was not expanding, so it silently skipped
the system-wide `known_hosts` — the file a managed Windows estate provisions
host keys into. A trusted host would have read as unknown, on Windows only, and
no Linux test could have seen it. Fixed, with the expansion under test.

 `ssh -G` resolves an unknown host and afterm reads
the answer; `ssh-keygen -F` finds a plain entry and pins its algorithm, refuses a
`@revoked` one, sends a `@cert-authority` one to the OpenSSH client, resolves
hashed and wildcard entries, and keeps ports apart. Those four shapes are exactly
what russh's own reader gets wrong, and only the real binary can show it. They
skip where there is no OpenSSH client and run on the Windows agent, where they
are the only evidence about Win32-OpenSSH's behaviour.

**Not automatable, and left as manual checks rather than pretended:** real 2FA
and jump hosts, the Windows OpenSSH agent over its named pipe, and terminal
behaviour on Windows.

### The interface, the terminal and the build

- **`F11` is full screen.** Claimed in the capture phase like every other
  chord, or xterm sends the key to the shell instead; unmodified only, so
  `Shift F11` and the rest stay the terminal's. The window is asked whether it
  is already full screen rather than afterm tracking it, because the window
  manager has its own ways in and out on every platform and afterm would not be
  told. `setFullscreen` is not in Tauri's default window permissions — reading
  the state is — so `capabilities/default.json` names it. In the browser
  preview the browser's own `F11` already is this and is left alone.
- **Carbon's sixteen carry hue; its chrome does not.** Two problems, and only
  one of them was contrast: on Carbon's own ground its blue read at 3.64 to one
  and its comment grey at 2.62, both under the floor Stockholm's test now
  enforces. The other was what the scheme could say. Sixteen greys could be
  told apart — the luminance ramp saw to that — but distinguishable is not the
  same as meaning something, and brightness cannot say which of two things is
  the error. In a `git status` the staged and the unstaged sets were two greys
  and only the headings said which was which.

  So the sixteen carry Stockholm's hues at two fifths of its saturation, on
  Stockholm's lightness: enough to tell a failure from a success, far short of
  a second coloured scheme. Six of them are warm, which is the rule this scheme
  was built on being given up — deliberately, and only there. The chrome is
  untouched and stays cool grey to the last token, because that is where the
  scheme's identity lives: the tabs, the sidebar, the status bar, the accent.
  afterm is grey; the host is not.

  The never-warm test in `src/theme.test.ts` is narrowed from `chrome +
  terminal` to `chrome` to match, and a new one states what would be lost if
  someone flattened the palette back: fifteen of the sixteen must carry hue,
  and bright white is exempt because pure white has nowhere to put one. The
  contrast floor now covers both schemes rather than Stockholm alone.

- **Stockholm's sixteen ANSI slots are chosen for the ground they sit on.**
  They were Terminus's VGA values — the primaries at full saturation — and they
  failed at the thing the sixteen exist for. Red read at 2.79 to one against
  the background, blue at 2.88, magenta at 3.48, and the grey used for comments
  at 4.08: four slots under 4.5, and the least legible colour in the terminal
  was the one that means something has gone wrong. They were also the loudest
  thing left in the window once the chrome's accent had been calmed, and their
  contrast spread from 2.79 to 9.58, which is what "some slots glow" looks like
  with a number on it. The replacements run 5.11 to 10.69. Hues come from
  palettes proven on a ground like this one rather than from a VGA adapter, two
  of them are the chrome's own tokens because the chrome had already answered
  the same question, and green is deliberately not the accent — a prompt the
  colour of the tab underline reads as selected. Two rules are now tests: every
  bright sits above its own normal, and every slot but black clears 4.5 on the
  ground. The preview shell grew `ls` and `git` screens to judge it on, because
  a swatch row says whether sixteen colours differ and nothing about whether
  they mean anything.
- **PowerShell and CMD are gone; every session is SSH.** Two thirds of the new
  session dialog opened a local shell that Windows Terminal does better, and
  the cost was spread everywhere else: an `isRemoteKind` branch on the status
  bar, the tab icon, the file pane and the telemetry command, a whole local
  filesystem module behind a pane only a local session could reach, and an
  address that had to be optional because a local shell had none. All of it
  went. `sysinfo` went with it — the only thing sampling this machine was the
  telemetry for local tabs — and the interface's own `SessionProfile` now
  requires a host, a user and a port, which is what stopped the status bar
  being able to read `undefined@host` at all. A local shell left in the
  restore store by an older version is dropped rather than reopened with
  nowhere to go, and a saved host with no address is dropped on load rather
  than taking the rest of the store down with it. A launch with nothing open
  now asks where to go, because there is no longer a local shell to open
  instead.
- **Unicode 11 character widths.** xterm measures against Unicode 6 tables by
  default, two versions before emoji were given widths at all, so an emoji or
  much of the CJK range took one cell here and two on the host: the cursor
  walked out of step by a column per character and anything drawn out of box
  characters came apart. One addon and one line.
- **Terminal flow control.** The interface counts the bytes it has been handed
  against the bytes xterm has finished parsing, and past a megabyte it holds
  the reader until it is back under a quarter of that. Before this the only
  queue between a fast producer and a slower renderer was the webview's own,
  which has no ceiling: `cat` of something large was a memory spike and a
  window that stopped answering. The gate holds reading only — writing is never
  gated, so `Ctrl C` still reaches the far end while the flood is held, which is
  how the flood gets stopped. A held pty leaves its bytes in the pty and the
  writing process blocks against a full one, exactly as it would against any
  terminal that stopped listening; a held channel stops adjusting its SSH
  window, which is the protocol's own way of saying wait.
- **Shell integration (OSC 133), where the host will send it.** The band and
  the pinned command were worked out from keystrokes, which reads an ordinary
  command well and cannot see a command recalled from history, a pasted one, or
  a prompt that spans lines — none of those arrive as typing. The terminal now
  reads the shell's own marks when they arrive, bands exactly what lies between
  the end of the prompt and the start of the output, and pins the command
  without the prompt in front of it. The heuristic stays for hosts that send
  nothing and stands aside by itself the moment a real mark arrives. Settings
  copies the snippet; afterm does not install it, because that would mean
  editing a file on a machine that is not afterm's to edit, or typing it in at
  login and clearing the screen to hide the echo — which would take the host's
  banner with it. The preview shell's `marks` command turns the marks on, which
  is the only way to see either path without a real shell on the other end.
- **The file pane edits, and takes a drag.** Files dragged in from Explorer
  upload to the directory on screen — Tauri reports the operating system's own
  drop, with real paths, which is the only way to learn a path at all. A file
  dragged out of the list downloads to the Downloads folder: a webview cannot
  hand a file to Explorer, because the operating system asks for the bytes at
  the moment of the drop and a file on another machine has none to give yet, so
  the gesture is taken for what it means rather than pretended at — and, since
  the pre-launch review, says so in the status line as the drag starts, because
  a gesture that cannot end where it looks like it will should say where it
  does end. Downloads never replace a file already in Downloads, and uploads
  never replace a remote file without the pane asking first; both travel under
  a temporary name until they are whole (`OPEN-ISSUE.md`). New folder
  is a toolbar button and rename is a pencil on the row, both inline editors
  like the tab and host names, and both refuse a name carrying a separator.
  Delete is the second icon on the row and asks first — a remote delete has no
  bin behind it — and a directory goes through `rmdir`, which refuses one that
  is not empty.
- **`PgUp` and `PgDn` page the scrollback**, on the normal buffer only. A
  full-screen program has its own idea of a page and is given the key.
- **The resize storm is gone.** The observer fitted and sent a window change on
  every callback, so dragging a divider was a `SIGWINCH` per frame and vim and
  htop repainted for each one. One fit per frame now, and one window change per
  size the far end has not already been told about.
- **The accent is no longer the terminal's own green.** It was `#00cd00` and
  `#00ff00` — fully saturated at both ends — which against a desaturated
  blue-grey ground made the primary button and the selection ring the loudest
  things in the window. Held back to about half saturation at the same hue,
  still above seven to one against the ground. The ANSI palette is untouched:
  those greens belong to the host's output, and a terminal that quietly
  restyles them is lying about what the program printed.
- **Nested terminal workspaces.** Each pane owns local tabs and can split left,
  right, up or down with `Ctrl Shift + Arrow`; combinations form a recursive layout.
  `Alt Shift + Arrow` moves focus, `Ctrl Tab` cycles within the focused pane, and new
  tabs join it. Dividers drag or resize by keyboard, empty panes collapse, and
  moving a tab preserves its xterm instance and scrollback. Panes carry no
  outline: side-by-side workspaces alternate between two grounds instead, on a
  checkerboard taken from their rectangles. The layout is restored with its
  sessions.
- **Downloads use Windows' Downloads known folder.** Redirected folders are
  honoured, and an unavailable platform lookup falls back to
  `<user>\Downloads` rather than the user-profile root.
- **The release build dropped thin LTO, on a measurement.** Builds 221 and 223
  differed in that one line: 7.4 minutes and 15.83 MB with it, 3.3 minutes and
  15.53 MB without. It was costing four minutes a build to produce a larger
  binary. The cargo cache key gained `Cargo.toml` at the same time, without
  which a profile change rebuilds everything and then silently declines to save
  the result, on every run from then on.
- **The frontend checks run on a host whose Node is too old.**
  `scripts/verify-in-container.sh` runs `tsc`, vitest and the production build
  in the official Node image, and `shoot` screenshots every colour scheme —
  window and Settings panel — in headless Chrome. Nothing is installed on the
  host and nothing outside the repository is written. It is what caught the
  preview shell writing Stockholm's green as a truecolour escape, which no test
  could see.
- **Terminal search** (`Ctrl F`). Finds anywhere in the 20,000 lines of
  scrollback, marks every hit and counts them, Enter and Shift-Enter to walk
  them. Ctrl+F is claimed before xterm sees it, or the shell would get a `^F`.
- **Latency is reported.** v2 promised this and had not delivered it. A ping is
  a no-op the protocol obliges the far end to answer, so it times the link
  rather than anything the host has to run — no shell, no process, nothing a
  loaded machine would slow down. Native transport only: the OpenSSH path's
  connection lives inside `ssh.exe`.
- **Sessions and workspace layouts reopen on launch:** tab profiles plus the
  split tree, focused pane and divider ratios, but no credential, connection
  state or scrollback. A restored tab connects the way it would have if opened
  by hand, which is why a password host still asks for one. Saving a host used
  to switch this off and show the connect dialog instead; the pre-launch review
  (2026-09-06) called that a surprise, and it is gone — the dialog opens
  unasked only when nothing was open. There is no control for restore in
  Settings, though an earlier version of this bullet said there was; the
  `afterm.restoreSessions` key in local storage is the only switch.
- **Directory listings are cached**, so walking back up a tree costs nothing.
  Refresh and a finished upload still go to the host, and a session that ends
  or reconnects drops everything it had cached — a host that has been out of
  contact is not one to trust a remembered listing from.
- **The tab state dot means one thing again.** Grey was both `connecting` and
  `closed`; there is no `closed` any more, because a session that ends is
  removed rather than left sitting there greyed out. Grey is waiting, green is
  up, red is trouble.
- **Ctrl-D closes the tab, and a dropped connection offers to reconnect.**
  Both used to leave a dead tab behind with the file pane stuck on
  "Connecting…". The backend now says which of the two happened — an exit
  status on the native channel, a process exit code on a pseudo-terminal — and
  a lost session keeps its tab, its scrollback and its place, with `R` to
  reconnect in place and `Q` to close.
- **The title bar is not stock white any more.** DWM paints the caption, its
  border and its text in the file pane's colours, following the scheme like
  everything else. The native frame is kept, so snap layouts and the window
  buttons are still Windows'. Windows 10 ignores the colour attribute and gets
  a dark caption from the window's theme instead.
- **A second colour scheme.** Every colour is a token in `src/theme.ts` and the
  stylesheet holds no literals, so *Carbon* — the same layout entirely in cool
  grey, terminal palette included — is a data change. Switching is live. The original
  scheme is now called *Stockholm*. Nothing paints a colour outside the scheme
  any more either: the preview shell and the spawn-failure message held
  Stockholm's values in truecolour escape sequences, and now name palette
  slots.
- **Settings holds everything that is not a session.** The colour scheme, the
  terminal font, the shortcut list and key installation were three sidebar
  buttons and two dialogs; they are one button and one dialog.
- **Terminal paste.** Suppressing the context menu left only Ctrl+Shift+V, the
  traditional terminal binding. Ctrl+V now pastes and Ctrl+C copies a selection
  — Windows expectations rather than terminal ones — while Ctrl+C with no
  selection still interrupts. Right-clicking in the terminal pastes too: the
  webview's menu stays suppressed and the terminal claims the button, reading
  the clipboard through a native command because WebView2 puts a permission
  prompt in front of the webview's own read.
- **Terminal throughput.** The base64 decode on the receiving end ran a callback
  per byte: 9 MB/s, and the ceiling on everything that scrolls. A preallocated
  loop makes it 274 MB/s. The WebGL renderer took the next ceiling, with a
  fallback to the DOM renderer when the driver takes the context away.
- **A crash no longer blanks the window.** React unmounts the whole tree when a
  cleanup throws; disposal is guarded and an error boundary catches the rest.
- **The path in the file pane has its own lines**, at a size worth reading: two
  of them, wrapping only at a separator so a name is never split down the
  middle. A path too long for both is cut from the front with `..` rather than
  trailed off at the end, because the tail — the directory you are in and the
  one above it — is the part being read.
