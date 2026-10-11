# How afterm works

The design behind afterm and the reasoning for it. What is built, change by change, is in
[`CHANGELOG.md`](../CHANGELOG.md); what is left is in [`ROADMAP.md`](../ROADMAP.md).

afterm speaks SSH itself. One authenticated connection per session carries the terminal, files and
telemetry, with the OpenSSH client kept as the transport for hosts whose configuration afterm
cannot honour faithfully — and reached for automatically. [`OPEN-ISSUE.md`](../OPEN-ISSUE.md)
records why, and what was verified to decide it.

## Behaviour

Nothing in the interface is placeholder data. A value afterm cannot measure renders as `—`, and a
listing it cannot read shows the error the backend returned, labelled with what kind of failure it
was: a refused credential offers a password prompt, an unreachable host offers a retry, and a host
afterm will not carry natively says so by name.

**A flood cannot take the window with it.** The interface counts the bytes it
has been handed against the bytes xterm has finished parsing, and past a
megabyte it holds the reader — in the pseudo-terminal where `ssh.exe` is
carrying the terminal, on the SSH channel's own window where afterm is. Nothing queues in the webview, which
has no ceiling of its own. Writing is never held, so `Ctrl C` still reaches the
far end while the flood is being held back, which is how the flood gets stopped.

**Shell integration is offered, not installed — and the panel says whether you
need it.** *Settings ▸ Shell integration* reads *Active* on a host whose shell
is already sending marks, and otherwise offers a snippet for `~/.bashrc` or
`~/.zshrc` that makes it send them. That state is not a setting afterm stores:
a shell that has sent an OSC 133 mark has the snippet, so the same signal that
stands the keystroke heuristic down is what the row reports. With the snippet,
the highlight and the pinned command follow the shell — through a command
recalled from history, a pasted one, a prompt that spans lines — and the pin
carries the command without the prompt in front of it. Without it nothing is
lost; the heuristic stays.

**A tab says when the agent in it is waiting for you.** afterm does not recognise the agent: over
SSH it runs on the host, where there is no process to see, and output that looks like one agent's
would change with every release of it. It reads the signal the agents already send a terminal to
ask for attention — OSC 9, OSC 777 `notify`, kitty's OSC 99, or a plain bell — and marks the tab
with an amber icon in place of its connection dot until the tab is looked at: a speech bubble
carrying the agent's message as its tooltip, or a bell when all that came was a bell. An icon
rather than a bigger dot, because a change of shape reads across a full strip where a change of
colour does not. A signal from the tab
in front of a focused window is not news and marks nothing, which is also what keeps a shell's
tab-completion bell quiet. With the window behind something else the taskbar button flashes too.
`OSC 9;4`, the progress report Windows Terminal draws on the taskbar, is not a request and is
ignored, or every background build would light its tab. Neither agent sends a signal afterm reads
by default — Claude Code's `auto` picks one only for terminals it recognises, and SSH does not carry
the terminal's name — so a tab is also marked when it goes quiet after working, an empty bubble
with nothing set up. Measured before it was built: Claude Code redraws its spinner and title at
least every 0.35 s for the whole of a turn and Codex its status line every 0.5 s, and both then
write nothing at all. So output that kept coming for two seconds and then stopped for three is a
turn that ended. The echo of typing does not count as output, and the watch is armed by typing
into the tab or looking at it and fires once per arming, so a login banner, a tmux status line or
a log that bursts now and then marks nothing. It cannot tell an agent's turn from a long build
finishing, and marks both. *Settings ▸ Coding agents* copies the one setting each agent needs to
say why it stopped, which puts its message on the tooltip and marks an approval it is waiting on.
Codex only signals when its terminal is unfocused, and a tab in the background reports itself
unfocused to a program that asks (`CSI ? 1004 h`), so that is exactly when it signals.

The file pane does not wait on any of that. It follows the shell on every
Linux host, snippet or no snippet: the telemetry stream already reads `/proc`
on the far end twice a second, and the shell's directory is two more reads on
the same channel — the sibling of that channel with a terminal is the shell,
and the process group in the foreground of that terminal is what the user is
typing at, which is the outer shell at a prompt and the nested one after
`bash` or `sudo -i`. The snippet's own OSC 7 still wins where it is installed,
because it arrives with the prompt rather than up to a poll later. Neither
path moves a pane the user has browsed somewhere else: what is followed is the
change, not the report.

*Add to this host* writes the snippet to `~/.bashrc` or `~/.zshrc` for you,
and says which file it chose and whether a login shell will actually read it —
bash reads `~/.bash_profile` at login, and a profile that does not source the
rc file leaves the snippet in place and inert. It is a button and never
automatic: it edits a file in someone's home directory on a machine that is
not afterm's, which is a thing to be asked for. *Copy snippet* is still there
for a host you would rather paste into yourself.

**One connection, one credential.** An SSH session's terminal rides the same
connection as its files and telemetry, so a password host asks once. A host
afterm cannot carry faithfully falls back to the OpenSSH client for the terminal
by itself, and the status bar names whichever is in use. Tick *Run the terminal
through the OpenSSH client* to force the classic behaviour, at the price of
being asked twice.

**Password-only hosts work**, confirmed on Windows against a stock Ubuntu
server. afterm opens its own authenticated connection and runs SFTP and
telemetry as channels on it, so a host that only accepts an interactive password gets the same file
pane and status bar as any other. The secret is used for that one handshake and dropped; there is no
field on a saved profile that could hold it.

## Architecture

**Terminal bytes bypass React entirely.** The backend emits bounded chunks straight to the
xterm.js instance; React renders the workspace tab strips, the sidebar and the status bar, and
never sees a byte of output. The active terminal in every workspace remains visible; inactive tabs
stay connected but unpainted. The backend owns the session and
performs resize, write and shutdown by opaque ID, whether that session is `ssh.exe` on a
pseudo-terminal or a channel on afterm's own connection — one enum, two shapes, one set of
operations. The pseudo-terminal shape carries the local shells too, and that is the whole of what
they cost: they have no remote half, so every command that needs a far end refuses in one place
rather than each caller asking whether this session has one.

**Workspaces are panes that own tabs, not a second window abstraction.** The layout is a recursive
tree of horizontal and vertical splits. `Ctrl Shift + Arrow` peels the active tab into a new pane in
that direction, `Alt Shift + Arrow` moves focus geometrically, and `Ctrl Tab` cycles only inside the
focused pane, `Ctrl Shift Tab` backwards through it. New tabs join that pane; closing its last tab
collapses the empty branch. Dividers constrain resizing to practical minimums when space permits,
accept arrow keys when focused, and balance on double-click.

A tab opens at the width it asks for and gives ground as more arrive, which is what a browser does
and what afterm did backwards. Tabs were content-sized between a floor and a ceiling, so a single
open tab was as narrow as its own name and seven were *wider*, because the widest name grew as tabs
were added. The width is a definite `width` with `flex-shrink` to take it back, and not a flex
basis, which looks equivalent and is not: a flex container's automatic size is built from its
items' content contributions and a basis does not feed one, so a basis left the strip measuring the
longest name and handing every tab that width — the same backwards behaviour, wearing the fix's
clothes. Four candidate rules were measured in a headless browser before this one was kept.

**A chord is claimed before the terminal can send it, and a focused terminal is not a text field.**
Every shortcut is taken on `window` in the capture phase and stopped there, or xterm would also see
it and send the escape sequence to the shell. The guard that keeps a chord from stealing a keystroke
from a rename box has to answer one question: is this a field somebody is typing into? xterm holds a
hidden `<textarea>` focused to take keys and drive IME, so a focused terminal answered yes — and
the two arrow chords, the only ones that consulted the guard, were waved through to the shell
rather than claimed. The rule now excludes anything inside the terminal surface, which is what
`src/lib/keys.ts` is.

What a chord costs is why both workspace chords carry two modifiers. A chord afterm claims is one
no remote program can ever see, so the question for each is what the terminal loses. Checked against
readline's built-in defaults — the bindings that survive an empty `inputrc`, not something a distro
added:

| Chord | Sequence | Bound by |
| --- | --- | --- |
| `Shift + Arrow` | `\e[1;2` | selection in editors and pagers |
| `Alt + Arrow` | `\e[1;3` | readline `backward-word`/`forward-word`; nano `scrollup`/`scrolldown`; fish `prevd`/`nextd` |
| `Ctrl + Arrow` | `\e[1;5` | readline `backward-word`/`forward-word` |
| `Ctrl Shift + Arrow` | `\e[1;6` | nothing |
| `Alt Shift + Arrow` | `\e[1;4` | nothing |

So splitting takes `Ctrl Shift + Arrow` and focus takes `Alt Shift + Arrow`, and every
single-modifier row is left to the shell. tmux is untouched either way: its `M-Arrow` and `S-Arrow`
bindings are prefix-gated, outside copy mode. Space is not a candidate for any of this, however
free it looks — a terminal sends on keydown, so the space would already be on the wire before the
arrow arrived, and afterm would be pausing the most-typed key on the keyboard to find out whether
it was one. The command palette runs every split without a chord at all.

**Every letter chord is `Ctrl Shift`, and that half is free.** The arrows were reasoned about
carefully and the `Ctrl`+letter set was not, which is where the whole cost had quietly gone:
`Ctrl B` is tmux's prefix, `Ctrl R` is readline's reverse history search, `Ctrl W` is
`unix-word-rubout` and vim's window prefix, `Ctrl V` is `quoted-insert` and vim's visual block.
Those are the four things people do most over SSH, and afterm was taking all of them.

Moving to `Ctrl Shift` costs the far end nothing, and that is a fact about the encoder rather than a
convention. xterm.js writes `Ctrl`+letter as a single control byte — `String.fromCharCode(keyCode -
64)` — and never consults `shiftKey` doing it; it implements neither `modifyOtherKeys` nor the kitty
keyboard protocol, the two things that could tell the halves apart. So `Ctrl Shift B` and `Ctrl B`
both arrive as `^B`, no remote program can bind one without the other, and the shifted half is
afterm's for free. It is also where GNOME Terminal, Konsole and xfce4-terminal already keep their
own chords, for exactly this reason.

Shift is only free on the letters. The same table gives `Ctrl`+digit the range `0x1B`–`0x1F` and
`Ctrl`+`[`/`\`/`]` the sequences `ESC`, `FS` and `GS`, all of them ignoring Shift as well — so
`Ctrl Shift 3` sends `ESC` and `Ctrl Shift \` sends `SIGQUIT`. Nothing is bound there, and nothing
should be.

Two chords stay unshifted on purpose. `Ctrl Tab` has no encoding at all — a terminal cannot send it
— so Shift is spent on the direction instead, and `Ctrl Shift Tab` cycles backwards. Font size keeps
`Ctrl +` and `Ctrl -`: xterm's table has no entry for `=` or `-`, so neither reaches the shell
shifted or not, and it is the zoom chord every browser and editor already uses.

Three unmodified keys are taken as well, which is a larger claim and is made deliberately in each
case. `PgUp` and `PgDn` page the scrollback, but only on the normal buffer: a full-screen program
has its own idea of a page, so `less`, `vim` and `htop` are handed the key untouched and only the
shell's own scrollback is afterm's to move. `F11` is full screen, unmodified only, the way a browser
does it — `Shift F11` and the rest stay the terminal's.

What the move buys back is worth naming. `Ctrl C` now interrupts in every case: it used to copy
whenever a selection existed, so a drag left by the mouse could silently eat the interrupt on a
runaway process, and the escape hatch was an undocumented `Ctrl Shift C`. That whole class of bug is
gone with the binding. The cost is `Ctrl V`, which Windows hands muscle memory to — right-click
paste covers it, and PuTTY, the incumbent for this audience, has never bound `Ctrl V` either.

**Workspaces have no outline. Two that touch stand on different grounds instead.** A border around
each pane is a line drawn to say something the shapes already say, and around a single pane it says
nothing at all — so there is none, and a lone workspace is just the window again. Each theme carries
three grounds close enough together that the window still reads as one slab: `--bg`, `--bg-alt` and
`--bg-alt-2`, each a handful of levels darker than the last, the same step the sidebar already
stands at against the terminal beside it.

Three, not two. The rule is the one the eye applies — two panes that share an edge should differ —
which makes it a graph colouring over rectangles rather than anything to do with the tree they came
from. It used to be the parity of a pane's column plus its row, a checkerboard, and a checkerboard
has only two colours to hand out. Two is not enough: a pane beside a split touches both halves, and
those halves must already differ from each other, so one of them was forced to match it — an odd
cycle, which is not two-colourable. That is the layout the shading got wrong, and it is a common
one. With a third ground the panes are taken in reading order and each gets the lowest shade none of
its already-shaded neighbours has, which resolves it. Touching at a corner does not count: two panes
diagonal across a grid meet at a point, and a point cannot be seen to be a seam, so a grid still
falls out as the checkerboard it always was.

Taking it from rectangles rather than from splits is what keeps identical layouts identically
shaded: one flip per step into a `second` child colours three panes in a row `0, 1, 1` or `0, 1, 0`
depending only on which way the splits happened to nest. Four mutually adjacent rectangles would
beat three shades, and greedy colouring is not optimal even where three would do; neither is
reachable by splitting the way afterm splits, so a pane whose neighbours have taken every shade
takes the one fewest of them are using rather than paying for an exact search.

The shade reaches the terminal itself, not just the chrome around it. xterm paints its background
into the canvas, so a CSS rule on the element behind it would stop at the tab bar and the terminal
would paint over the rest; a shaded pane gets its own `ITheme` with its ground, and `cursorAccent`
with it, or the cursor block reads as a smudge rather than a hole in the text. Stockholm's ANSI
black moved down a step to make room, because the third ground landed on the value it used to hold
and black is the slot programs reach for as a background. Two things follow from the top-left pane
being first in reading order: it is never shaded, so the pane against the sidebar always contrasts
with it, and the single-pane case costs nothing. What the border used to say about focus is said by
the active tab now, which carries an accent rule in the focused pane and dimmer text everywhere
else. Dividers lost their resting line for the same reason the frames did — the shade boundary is
the divider — and still light up under the pointer.

Terminal components stay in one flat, keyed React list while their rectangles change. Moving a
tab therefore does not remount xterm, restart the session or discard scrollback. The focused pane
drives the global file browser and status bar.

**The throughput ceiling is in the webview, not in Rust.** Reading a pipe costs nanoseconds; what
costs is what happens to the bytes afterwards. Three things were worth fixing and are worth not
undoing. Decoding a chunk with `Uint8Array.from(atob(data), c => c.charCodeAt(0))` runs the iterable
protocol with a callback per byte: 3.4 ms for one 32 KiB chunk against 0.11 ms for a preallocated
loop, or 9 MB/s against 274 MB/s. And xterm's default renderer builds DOM nodes per cell, so the
WebGL addon carries the drawing — falling back to the DOM renderer if the driver takes the context
away, which Windows does on sleep, driver updates and remote desktop. Its drawing buffer is
preserved because several visible panes can be resized or composited without holding keyboard
focus; otherwise an unfocused canvas can flash blank between redraws.

And the third: bytes handed to xterm are not bytes drawn. It parses on its own schedule and says so
through a write callback, so the interface counts what it has been given against what has been
finished, and past a megabyte it closes a gate on the reader until the backlog is under a quarter of
that. Without it the only queue between a fast producer and a slower renderer is the webview's own,
which has no ceiling — `cat` of something large was a memory spike and a window that stopped
answering rather than a terminal that scrolled for a while. The gate holds reading only. Writing is
never gated, so `Ctrl C` still reaches the far end while a flood is being held, which is how the
flood gets stopped. A held pseudo-terminal leaves its bytes where the operating system wants them
and the writing process blocks against a full one, exactly as it would against any terminal that had
stopped listening; a held channel stops adjusting its SSH window, which is the protocol's own way of
saying wait.

**Character widths come from Unicode 11, not Unicode 6.** xterm's default tables predate emoji
having widths at all, so an emoji or much of the CJK range occupies one cell in the renderer and two
on the host: the cursor walks out of step by a column per character, and anything drawn out of box
characters comes apart down the middle. One addon and one line, and it is the kind of correctness
that is invisible until it is wrong.

**One broken component does not take the window.** React unmounts the whole tree when a render or a
cleanup throws, and a blank window is a worse failure than whatever caused it. Renderer disposal is
ordered and guarded, and an error boundary catches the rest: a bug costs a message and a reload
rather than every tab on screen.

**Two transports, one interface.** An SSH session is carried by one of these; the file pane, the
status bar and the terminal cannot tell which, and the status bar names it so the user can. The
native one is the default and the OpenSSH client is reached for automatically, per host, for
anything afterm will not reproduce faithfully.

*Native.* One `russh` connection per session. SFTP is a subsystem channel on it, telemetry is an
exec channel streaming a few short `/proc` reads on an interval, and a native terminal is a PTY
channel. One handshake, so a password is asked for once and dropped unless you asked afterm to keep
it; no login per click, no audit
line per navigation, and transfers can report progress because the client is inside afterm.

*OpenSSH client.* The `sftp` client run once per operation, plus a long-lived `ssh` streaming
`/proc`. This is not a legacy path: it is what carries hosts whose configuration afterm will not
pretend to reproduce — `ProxyJump`, `ProxyCommand`, `HostKeyAlias`, an SSH certificate authority.
Those hosts are named in the interface rather than quietly downgraded.

The OpenSSH path runs the client once per operation because an earlier design kept a single
`sftp -b -` process alive and framed responses inside its output stream, and that could not be made
to work across platforms. Batch mode enforces quiet mode, so the `sftp> ` command echo is documented
to be suppressed, and the client's stdout is block-buffered when it is not a console — neither
behaviour matches between builds, which produced three Windows-only failures from assumptions
verified only on Linux. Closing stdin makes the client run its batch and exit, and a process that
exits flushes its buffers, so afterm reads everything it wrote and parses it whole. Nothing depends
on prompts, echoes, ordering or flush behaviour. Both helpers connect with `BatchMode=yes`, so they
fail visibly instead of hanging on a prompt nobody can see, and both are spawned with
`CREATE_NO_WINDOW`, without which Windows flashes a console window for every helper process.

**afterm never guesses what the user's client believes. It asks.** `ssh -G` resolves the effective
`ssh_config` — aliases, wildcards, `Include`, `Match`, the system file — and `ssh-keygen -F` resolves
`known_hosts`, including hashed entries, wildcard patterns and the `@revoked` and `@cert-authority`
markers. Both are run to completion and read whole, the same discipline the SFTP client is driven
with and the opposite of the interactive-stream parsing that broke four times on Windows.

russh ships its own `known_hosts` reader and it is not used: it silently skips markers, does not
match wildcards, and reports a key of an unexpected algorithm as an unknown host. The differences
are tabulated in `OPEN-ISSUE.md`.

Reading the answer means resolving what it contains. Win32-OpenSSH prints its system paths with a
literal `__PROGRAMDATA__` placeholder, and a path afterm cannot resolve is one it silently skips —
which for the system-wide `known_hosts` means a trusted host reading as unknown. `ssh -G` also
prints values already parsed, so the quoting that separated two paths from one path containing a
space is gone; the filesystem is the tiebreak.

afterm draws no title bar of its own. The native one is kept — snap layouts, the real window
buttons and every resize behaviour come with it, and none of them is worth reimplementing — but
Windows paints it, so it is the one surface the stylesheet cannot reach, and a stock white caption
above a dark window is a seam in a design that is otherwise one slab. `DwmSetWindowAttribute` closes
it: the caption, its border and its text are set to the file pane's own colours whenever the scheme
changes, because the bar sits directly above the sidebar and reads as the top of it. That attribute
is Windows 11 and up, so the window also asks for the dark theme in `tauri.conf.json`, which Windows
10 does honour — there the caption is dark rather than exact, and never white. Below it, tab strips
that touch the top edge double as the drag region. Sidebar, terminal and status bar share one
background, and workspaces that touch each other stand on two alternating shades of it rather than
being boxed in; dividers appear only where the user has created a split, and the workspace holding
the keyboard is named by the accent rule under its active tab. Everything the bar used to hold
lives in the command palette (`Ctrl Shift Space`) and behind the sidebar's single footer button: the
colour scheme, the terminal font, the shortcut list and `authorized_keys` installation are all
inside Settings.

**The chrome is sized against the display, not against the window.** A CSS pixel is not a fixed
physical size, and the file pane, tab strip and status bar were drawn for a 1080p panel: on a 4K one
at 100% the same pixel covers a quarter of the glass, and the type comes out at half the height it
was designed at. `src/lib/scale.ts` sets `--ui-scale` from `screen.width` and every size that has to
grow together is expressed through it — type, the rows it sits in, the icons beside it, and the
pane's own width, since bigger text in a pane that stayed put would only crowd it.

The display rather than the window, because it is the display the text is physically small on: a
half-width window on a 4K panel needs exactly what a maximised one needs, and a width media query
would hand it the 1080p sizes. And `screen.width` is already in CSS pixels, so Windows' own display
scaling has been applied before afterm sees it — a 4K panel at 200% reports 1920 and is left alone
rather than scaled a second time on top of what Windows already did. The tokens carry a `--ui-`
prefix, which is also the rule that keeps them out of the stylesheet's colour test: a theme has no
opinion on how wide the file pane is.

The terminal's own font is not on this scale. It is a setting, on `Ctrl +` and `Ctrl -`, and
scaling it would fight the preference — but note that changing it *is* a resize. The pane keeps its
pixels and gains or loses rows and columns inside them, so the far end has to be told, exactly as it
is when the window or a divider moves. It was not, once, and the shell went on wrapping to the old
width until an unrelated resize told it by accident.

**Colour lives in one file.** `src/theme.ts` holds both schemes — a chrome palette written as CSS
custom properties, and a terminal palette written as xterm's own. The stylesheet has no colour
literal in it at all, so a scheme is a data change rather than a stylesheet edit, and the token list
is a TypeScript union: adding a token is a compile error until every scheme answers it. A test reads
the stylesheet back and fails if it reads a property no scheme sets, or if a scheme sets one nothing
reads. Switching is live, which is why the input band is redrawn on a switch and not only
recoloured: a decoration's colour is fixed when it is registered, and xterm paints the part of the
band that sits under the glyphs into the canvas, where CSS cannot reach it.

*Stockholm* is the original, on a blue-tinted ground. Its sixteen ANSI slots were Terminus's VGA
values and are not any more: the job of the sixteen is semantic — red fails, yellow warns, blue
recedes, dim is comments — and four of them could not be read on this ground, error red worst of all
at 2.79 to one. The replacements run 5.11 to 10.69 and every one clears 4.5, with each bright its own
normal made lighter rather than a second, louder colour. Two are the chrome's own tokens, because the
chrome had already answered the same question: ANSI yellow is `--warn`, bright red is `--danger`.
Green deliberately is not the chrome's accent, or a prompt would read as selected. `src/theme.test.ts`
holds both rules.

*Carbon* is the same layout with the hue taken out of the chrome — every token in it, to the last
one, and that is where the scheme lives: the tabs, the sidebar, the status bar, the accent. Its
sixteen ANSI slots are the exception, and deliberately. They were grey too, laid out as one
luminance ramp so they could be told apart, and they could — but being distinguishable is not the
same as meaning something. Brightness can say two things differ; it cannot say which of them is the
error. In a `git status` the staged and unstaged sets were two greys and only the headings said
which was which. They now carry Stockholm's hues at two fifths of its saturation: enough to tell a
failure from a success, far short of a second coloured scheme. afterm is grey; the host is not.

Carbon's grey is cool rather than neutral. Every value in it was once R = G = B, which is grey by
the numbers and brown to the eye — a neutral dark surface reads warm, and that warmth was what made
the scheme look muddy instead of sleek. The ramp now carries a single hue near 213 degrees, red
dropped and blue lifted with green a little under half way between, spreading five steps at the
darkest to nine at the brightest. Stockholm's ground spreads eighteen and reads as blue; a quarter
of that reads as grey, which is the point — enough to take the warmth out, not enough to name a
colour. Pure white and pure black are left alone, having nowhere to go, and anchor the ends.

**The clipboard is read natively.** WebView2 treats `navigator.clipboard.readText` as a permission
request and answers it with a prompt, which is not something to put in front of every paste, so
right-click paste goes through a Rust command backed by `arboard` with its image support compiled
out. `Ctrl Shift V` goes the same way, and has to: the webview cannot be handed the key instead,
because Chromium spends `Ctrl Shift V` on paste-and-match-style and `Ctrl Shift C` on the element
picker. Both are taken from the webview with `preventDefault` and from xterm by returning false, and
the clipboard is worked directly. Writing needs none of it — `navigator.clipboard.writeText` is not
permission-gated — so the copy half goes through the browser, as the settings panel already did.

**A pasted image goes to the host, and its path comes back.** A coding agent in an SSH session
reads the clipboard of the machine it runs on, which is the host, while the screenshot is on this
one, and no terminal key can reach across that. What the agent does accept is an image's path in place of a paste. So when the paste key
finds no text but an image, an SSH tab sends the image to `~/.cache/afterm/paste` over the
session's own transfer, and pastes the path. Text always wins: Excel and browsers put a picture
beside the text they copy, and it is the text a terminal wants. The image is read as the PNG bytes
the copying program left — Greenshot, the Snipping Tool and browsers all leave one — and only a
bare bitmap, which is what the Print Screen key leaves, is converted; `arboard`'s image support
would decode every image to pixels to do the same, and bring a colour-management stack with it.
The folder is made `0700`, because a screenshot is as likely as anything to show what other
accounts on a shared host should not see, and pastes older than a week are cleared out of it after
each new one, by name, so nothing else in it is ever touched.

**A remembered password lives in the operating system's credential store, and nowhere else.**
Password hosts are what afterm's own transport exists for, and typing the same password into every
new tab is the cost of that. So a *saved* host can offer to remember it: ticked on the password
dialog, written after the host has accepted the password, and read back inside Rust into the
handshake that needs it. There is no command that returns a saved password, so there is nothing for
the webview to leak; `profiles.json` gains no field, and the profile's own id is the key the entry
is stored under.

The rule that does the most work is what the entry is bound to. The stored blob carries the
account, the resolved hostname, the port and the host key fingerprint alongside the password, and
all four are checked before it is used. A saved host repointed at another machine, or one that comes
back presenting a different key, is asked about rather than handed the old password — and the entry
is left alone, because it is still correct for wherever it was saved for. The binding travels
*inside* the entry rather than beside it in afterm's own files, which is what makes it worth
checking: a binding in `profiles.json` could be edited by anything that can write the file.

It is tried once per attempt. A password the host refuses counts against that host's own attempt
limit, so a stale saved password must not be walked past the server again by this tab retrying or
by the four other tabs on the same host — the refusal is recorded once, for the process, and the
dialog that follows says the host refused it rather than looking like afterm forgot. The attempt
sits *ahead* of the check for whether afterm may raise a dialog at all, which is deliberate: using
a remembered password asks nobody anything, so a two-second telemetry poll and a click on the file
pane both connect on one where before either had to give up and wait for someone to type it.

Where there is no operating-system store there is no feature. The settings panel says so and the
checkbox is not offered; an encrypted file with a key in the executable is an obfuscated credential
store rather than a credential store, and shipping one would make the promise this is all built
around untrue. Windows Credential Manager is the store today, written with
`CRED_PERSIST_LOCAL_MACHINE` so the credential stays on this machine for this Windows user rather
than roaming. macOS is one dependency and one `Store` implementation in `secrets.rs` when it comes:
nothing above that module names a platform, and the interface asks whether saving is available and
why not, never which operating system it is on.

What the store protects against is worth stating plainly, and the settings panel states it: another
account on the machine, yes; anything running as you, no; an administrator of the machine, no.

**Copy on select is on, and it says so every time it fires.** PuTTY copies whatever the mouse
highlights, every X11 terminal does, and enough of this audience arrives expecting it that leaving
it out reads as a missing feature rather than a decision. It shipped off by default all the same,
because turning it on means every drag spends the clipboard: highlight a line to read it and the
address you had copied to paste into the next command is gone. The objection was never to the
copying — it was that the copying was silent, and a clipboard that empties without a word is a bad
surprise however useful the feature is.

So the drag now answers for itself. A successful write puts *Copied to clipboard* in the bottom
corner for a moment and then takes it away, which turns the silent overwrite into something the
user watched happen: nobody has to wonder later where the address went, because they saw it go.
With that in place the default flips to on, which is what the audience this is built for expects to
find already working, and the setting stays in *Settings* for anyone who wants the old behaviour
back. Turning it off is remembered; having never touched it is not the same answer, which is why
the stored value is read as "on unless it says `false`" rather than compared against `true`.

The corner message is for copy-on-select alone. `Ctrl Shift C` is a key somebody pressed on
purpose and needs no receipt; this is the copy nobody asked for out loud, and it is the one worth
announcing. It is also only shown when the write actually landed — the clipboard can refuse, and
announcing a copy that did not happen would send someone off to paste nothing.

The hook it does *not* use is xterm's `onSelectionChange`, which looks like the right one and is
wrong twice. It fires for every cell the mouse crosses, so one drag down a screen of output is a
hundred clipboard writes for ninety-nine selections nobody meant to keep. And it fires for
selections the mouse never made — the search addon selects each hit it lands on, so `Ctrl Shift F`
and a walk through the results would quietly replace the clipboard with whatever the last match
was. So the copy hangs off the end of the drag instead: left button down on the pane, and the
release. The release is heard on the window rather than the pane, because a drag that starts in a
terminal can finish over another pane, over the sidebar or past the edge of the window and is an
ordinary drag in each case; the button-down flag is what stops an unrelated click elsewhere from
copying a selection a pane is still holding. The right button is left out of it, or every
right-click paste would answer itself by putting the selection back on the clipboard it had just
read.

**A saved host is called whatever you call it.** The connect dialog takes an optional name, and any
saved host can be renamed in place on its *Saved hosts* tab; the address stays on the row underneath, so a
name never hides where a session actually goes. Leave the name empty, or clear it, and the host goes
back to being known by its address — the absence of a name is not an error to reject, and a blank row
nobody can pick out of a list would be the worse outcome. The name is a label on the profile, so it
titles every tab opened from that host afterwards and is what the command palette offers; renaming a
tab is still separate, because a tab is one session and a saved host is all of them.

**A launch is an empty window.** afterm used to reopen the tabs and the split it closed with, keeping
open tabs in `localStorage` as profiles and the split tree as ids, focus and ratios. It no longer
does, and the reason is what a tab is here: not a document but a connection. Reopening one is not
restoring a view, it is dialling a host — so a launch fired off however many handshakes the last run
happened to end with, and asked for however many passwords those wanted, before the window had been
looked at. Closing afterm is how you end a session, and the window that comes back is blank with the
connect dialog on it. The keys that carried the old state are removed at startup rather than left to
sit there, because they hold the host and user of every tab an earlier version had open. What is
still remembered between runs is the theme, the terminal font size and the saved hosts — a scheme, a
size, and a list you wrote on purpose.

**Saved hosts are in the dialog, not the sidebar.** They were a pane behind a tab strip that split
the sidebar in two, and the split was permanent for a list read once at the moment a session begins
and never again — while the file pane, which is read for as long as the session lasts, had to share
the space with it. The sidebar is the files now, and the hosts are the first tab of `Ctrl Shift N`, which
is the moment they are wanted. The first nine carry a number, so a host is one keystroke from the
dialog opening; the list takes focus when the tab does, or a digit typed at it would go to the
terminal behind the dialog instead. The rename and forget controls came with the list, so nothing
that lived in that pane was lost with it.

**The local shells are buttons, not a third kind.** PowerShell and Command Prompt sit at the far end
of the connect dialog's tab strip, styled like the two tabs beside them and behaving like the
buttons they are: pressing one opens the shell and the dialog closes, because there is no second
screen behind either. That is the entire feature. It was two thirds of the dialog once, with a form
and a saved profile and telemetry of its own, and the cost was not the shells — it was `isRemote`
in the status bar, the tab icon, the file pane and the telemetry command, and an address that had
to be optional on every profile because one kind of session had none. So the address is still
required and is empty for a local shell, one predicate answers *is this a shell on this machine*,
and the three screens that are about a far end say nothing at all rather than something invented:
the status bar goes blank, the file pane says why it is empty, and installing a public key is
disabled. Nothing about a local shell is saved, so nothing has to be migrated when they go again.

**The sidebar's top row is the host.** It used to say `FILES` over a pane that is plainly a file
pane, with the host on a second row underneath — two rows spent on one fact, and 30px taken off the
listing to say nothing. The host sits on the sidebar's own top row now, beside the button that
collapses the pane, and everything below it starts that much higher.

**The path is two lines, and what it drops is the head.** A line may only wrap after a separator:
every piece between separators is rendered unbreakable, which is what stops a browser splitting
`customer-integrations` at its hyphen, and the `<wbr>` between them is the only break opportunity
left. Two lines are held whether the path needs them or not, so the listing below does not jump as
you walk into a longer name.

A path too long for both lines is cut from the front — `/srv/deployments/releases/config` shows as
`../releases/config` — because the tail is the part being read: the directory you are in and the one
above it say where you are, and the root does not. Which pieces go is measured rather than
calculated, one at a time until the tail fits, because whether a piece fits depends on the font and
the width of the pane, and dropping one changes where every piece after it wraps. The measurement
counts distinct line offsets rather than comparing heights, so a horizontal scrollbar stealing the
box's height cannot be mistaken for a third line; it runs in a layout effect, so the loop settles
before the browser paints; and it terminates because the count only ever climbs and stops at the
last piece. The whole path stays on the tooltip. The parent-folder button moved down into the
toolbar, which is where the second line's room came from.

The toolbar is parent, refresh and Download, with everything that adds to the directory — Upload
files, Upload a folder, New folder — behind a `⋯` at its end that opens on hover or a click. Six
buttons did not fit the pane at its narrowest, and those three are the ones used without thinking.

Neither wrapping nor eliding can answer a single name wider than the pane, with nowhere in it to
break, and that alone is what the scrollbars are for.

This replaced a marquee that scrolled the path back and forth on one line, and which had a rule
disabling itself under `prefers-reduced-motion` — a path that will not fit is now a path on two
lines, which needs no motion to read and cannot be turned off underneath itself.

**The file pane edits, and takes a drag.** Files dragged in from Explorer upload to the directory on
screen. The page never sees those: Tauri intercepts the operating system's own drop and reports it
with real paths, which is the only way to learn a path at all — a browser's drop event hands over
content, not a location — and the position it reports is in physical pixels, so it comes back down
through the display scale before it is compared with a layout box. A file dragged out of the list
downloads to the Downloads folder. A webview cannot hand a file to Explorer: the operating system
asks for the bytes at the moment of the drop and a file on another machine has none to give yet, so
the gesture is taken for what it means rather than pretended at, and the status bar says where the
file went. New folder is in the toolbar's `⋯` menu and rename is a pencil on the row, both inline editors
like the tab and host names, and both refuse a name carrying a path separator — the editors ask for
a name inside the directory on screen, and a separator would quietly put the result somewhere else.

**A click selects; a double click opens.** A directory used to open on the single click, which left
no way to hold one — and so no way to download one, because the toolbar acts on the selection. Every
other file manager has worked the other way round for thirty years, so the pane now does too, with
Enter opening from the keyboard and the first folder click of a run saying where the gesture went.

**Whole folders, both ways.** One walk, read in either direction, over whichever transport the
session already has, one file at a time — both transports can list a directory, fetch a file, make a
directory and send a file, and neither should be able to disagree about what a folder transfer
means. It never follows a link: that is how a copy escapes the folder it was asked for, and how a
loop runs until the disk is full. Coming down, every remote name goes through the same reduction to
one plain component a single download uses and is then created exclusively, so a second copy of
`reports` lands in `reports (1)` and a folder already in Downloads is never written into. One file
that cannot be read costs that file rather than the tree, and the status line reports what arrived
rather than a bare success: *Downloaded 12 files from reports to …Downloads/reports — 2 entries
skipped, 1 could not be read*.

Going up, a folder arrives by drag or by *Upload a folder* in the toolbar's `⋯` menu — a system dialog offers
files or directories, never both. A name already taken on the host is the host's own refusal, not a
guess from the listing on screen: `mkdir` is an exclusive create on both transports, and a refusal
followed by a look at the name is what tells a taken name from a real error. The question that
raises is asked about the folder, once — *Merge*, Keep both, Skip. Keeping both takes the next free
name and refusing sends nothing; merging is the only answer under which a file on the host is
written over, and each of those still goes through the single-file replacement, staged and moved
into place wearing the permissions of what it replaced. What all of that trades away is under "Known
limits" in `ROADMAP.md`.

Delete is the second icon on the row, and it asks before it acts: there is no bin on the far end and
nothing to undo with, so the row turns into *Delete* and *Cancel* and waits. A directory goes with
everything in it, as it does in WinSCP, and its button says *Delete all* so that is known before it
is pressed. SFTP has no recursive remove, so the pane walks the tree: files and links first, then
directories deepest first. A link is removed as a link and never walked into, so what it points at
survives. Anything that will not go is left in place with its parent, and the first refusal is
what the status line reports.

Directory listings are cached only in memory, per session and path, so walking back up a tree costs
nothing. Refresh and a finished upload go to the host regardless, and a session that ends or
reconnects drops its cache, because a host that has been out of contact is not one to trust a
remembered listing from. Downloads resolve Windows' Downloads known folder rather than dropping a
file in the user-profile root; a redirected folder is honoured, with `<user>\Downloads` as the
fallback.

**A transfer never replaces anything without being told to, and never shows half a file.** The
remote name is not trusted to be a name — on Windows `..\x` is a path — so the backend reduces it to
one plain component under Windows' rules and creates it with `create_new` inside Downloads; the
interface has no say in the destination. Downloads travel as `name.part` and are moved to their real
name only when whole, taking `name (1)` if the name is already there. Uploads travel through a staging
name nobody can prepare in advance — the protocol's exclusive flag on afterm's own transport, a
`mkdir` that refuses an occupied name on the other — and are moved into place with a `rename`
OpenSSH refuses when the target exists; a name already taken makes the pane ask — Replace, Keep
both, or Skip — and it asks on the backend's refusal, not on the listing, so a stale listing cannot
skip the question. A transfer that fails removes its temporary.

**A session that ends and a session that drops are not the same event**, and afterm had been
treating them as one. A shell that ends on its own — `exit`, Ctrl-D, a logout — takes its tab with
it, which is what every terminal does and what leaving a dead tab behind was failing to do; the file
pane used to sit on "Connecting…" underneath it, forever, for a session that had already ended. A
connection that drops keeps its tab, because the session it carried is still what the user asked
for: the pane says so over the scrollback that was on screen when it went, and offers `R` to
reconnect in place or `Q` to close.

It also reconnects by itself: every five seconds for five minutes, counted from when the connection
went, with a countdown and the last attempt's error on the popup and `S` to stop. The next attempt
is scheduled when the previous one has finished rather than on a fixed clock, so a host that takes
its time to time out, or a password dialog left open, never has two attempts stacked on it. Only an
unreachable host is retried: a refused credential, a changed host key or a cancelled password
dialog ends the retrying, because waiting fixes none of them and a credential offered every five
seconds is how accounts get locked. Through the OpenSSH client an attempt counts as connected as
soon as `ssh.exe` is running, before it has reached anything, so a session has to stay up for ten
seconds before a drop counts as a new outage with five minutes of its own. Each attempt builds a new
terminal, and the old one's text is carried into it, dimmed, so the screen that explained the drop
is still there to read.

Telling them apart is the backend's job, and each transport has its own answer. A shell on the
native connection sends an exit status or an exit signal before the channel closes; a connection
that drops sends neither. On a pseudo-terminal the process is waited on for its code, which is why
the reader thread owns the child and the session keeps only a killer — a thread blocked in `wait`
cannot also be the one that kills. There, OpenSSH's own convention decides: 255 is `ssh.exe` saying
the connection failed, and anything else came from the remote side.

**Typed input** gets a background band and an accent edge, and is remembered so it can be pinned to
the top of the viewport once its output scrolls past. Where the band goes is worked out two ways.

The shell says so, when it will. OSC 133 marks the start of a prompt, the end of it, and the start
of a command's output; afterm bands exactly what lies between the last two and pins it without the
prompt in front of it. *Settings ▸ Shell integration* copies the snippet that makes a shell emit
them, and afterm does not install it: the alternatives are editing a file on a machine that is not
afterm's to edit, or typing it in at login and clearing the screen to hide the echo, which would
take the host's banner with it. The first mark a session sends is reported up to that panel, so the
row offers the snippet only to a host that has not got it and reads *Active* on one that has.

Otherwise afterm infers it, by anchoring a marker at the cursor when a keystroke run begins and
closing it on Enter. That reads an ordinary command well and cannot see three common ones at all —
one recalled from history, one pasted, and a prompt spanning lines — because none of those arrive as
typing. The inference stands down for good the first time a real mark arrives, so the two never
band the same row twice. Both suspend inside full-screen programs, where the screen is repainted
freely and line positions carry no meaning.

## Security notes

- **No credential is offered to a host that fails verification.** The native connection checks the
  host key against `known_hosts` before authentication begins; a key that is unknown, changed or
  revoked aborts the connection. The test battery counts credential checks on a refused connection
  and requires zero.
- **afterm does not accept host keys.** It only verifies. A host with no `known_hosts` entry is
  refused with an instruction to connect the terminal and accept the key there, where the prompt is
  the one the user already knows.
- **Host key algorithms are pinned to what is recorded.** Otherwise the connection can negotiate a
  key type the user's OpenSSH never saved, and a long-trusted host reads as unknown.
- **A credential is used for one handshake and dropped.** It is never written to a profile — there
  is no field for one — never logged, and never held for a later operation. The renderer, the IPC
  layer and russh each handle it transiently, so the honest claim is that afterm does not retain it,
  not that no copy of it ever existed.
- **A timer never raises a credential dialog.** Only a deliberate action can; the two-second
  telemetry poll gets a typed failure back and the interface offers a button.
- **Offering keys leaves room for the password.** Every rejected credential counts against the
  host's `MaxAuthTries`, so afterm stops offering keys before it has spent the budget a
  password-only host was always going to need.
- **The RSA signature is negotiated, not assumed.** Signing an RSA key with SHA-1 is what the
  underlying library does unless told otherwise, and OpenSSH has refused `ssh-rsa` by default since
  8.8 — so an RSA key would be rejected by every current server and the user handed a password
  prompt. afterm asks the host which signature it accepts.
- SSH arguments are passed directly to a process builder, never through a command shell.
- Host and username fields reject whitespace and option-prefix injection, in the interface and again in the backend.
- Paths sent to `sftp` reject embedded quotes and newlines.
- OpenSSH performs normal `known_hosts` verification.
- **The configured cryptographic policy is honoured.** `Ciphers`, `MACs`, `KexAlgorithms`,
  `HostKeyAlgorithms` and `PubkeyAcceptedAlgorithms` are read from `ssh -G` and intersected with
  what afterm's own transport can negotiate, in the configuration's order. An empty intersection is
  never widened back to a default: the host is refused by directive name and stays on the OpenSSH
  client. The same list governs a rekey, not only the first handshake.
- **An upload is private for the whole of its transfer, not from the end of it.** afterm's own
  transport creates the staging file `0600` in the `open` request, so it never exists under a wider
  mode at all. The OpenSSH-client transport cannot pass attributes to the create `put` performs, so
  it stages inside a directory of its own instead: `mkdir`, `chmod 700`, and only then `put`. This
  is a fix, not a design note — the previous order narrowed the staging file the moment the
  transfer ended, which left a `0600` remote file being replaced from a `0644` local one readable
  to anyone who could reach the destination directory for as long as the transfer took. An
  unguessable name does not help there: a name is discoverable by listing, and a directory nobody
  may enter has nothing to list.
- **A replacement keeps the permissions of what it replaced.** The destination's own mode is
  applied to the staging file before it is moved into place, so the file never exists under its
  real name wearing the wrong one. A destination carrying setuid, setgid or the sticky bit is
  refused rather than replaced.
- **Nothing an upload cannot finish is left behind.** A failed transfer, a refused chmod and a
  refused commit all go through the same cleanup; when a disconnected host makes even that
  impossible, the failure names the staging path rather than leaving it to be found.
- **The terminal reads and writes on separate tasks.** One task doing both deadlocks against any
  host that answers while it is being written to: the write parks waiting for send window, the
  channel's receive queue fills because nothing is draining it, and the window adjustment the write
  is waiting for is behind the block. Queued input is bounded, and closing a tab overtakes a write
  that cannot finish rather than queueing behind it — and does not then wait on the far end being
  told, because a host that has stopped reading its socket blocks even that.
- **A session is not reported as connected until the host has said so.** `request_pty` and
  `request_shell` submit a request; the acknowledgment is a separate channel message, and a host
  that refuses one used to produce a green status light over a terminal that would never show
  anything. Both answers are awaited, with a timeout, and a refusal takes the channel down with it.
- **A saved host is not lost to an entry beside it.** The store is decoded entry by entry, so
  nothing this version cannot read costs the hosts it can; an entry it cannot read is carried
  through the next save rather than dropped, and a store that cannot be understood at all is
  reported and left alone rather than answered with an empty list the next save would make true.
- **Telemetry and diagnostics are bounded.** Both transports reassemble the metric stream through
  one parser with limits on line length, block size and row count — including empty rows — and
  every capture of a helper's output is capped while still being drained, so a broken or hostile
  host can neither exhaust memory nor deadlock a helper on a full pipe.
- Public-key installation validates the key type, base64-encodes its contents, and never transfers a private key.
- **Public-key installation runs on afterm's own connection**, not by typing into the terminal, so
  it does not depend on what is in front of the terminal and cannot be captured by an editor or
  redirected by a `sudo -i` or a nested `ssh`. The host's exit status decides whether it worked, and
  the account and file it reports back are the host's own answer. It is unavailable on sessions
  carried by the OpenSSH client, which is said rather than half-done.
