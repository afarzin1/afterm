# afterm

afterm is a Windows-first SSH client focused on fast remote workflows, a calm interface, remote
files, and useful host telemetry: terminal, file browser and live server stats in one workspace.

afterm is free and open source under the [MIT license](LICENSE). No account, no license key, no
trial, nothing held back.

It is for SSH. There are local PowerShell and Command Prompt tabs, and they are second-class on
purpose: two buttons at the end of the new-session dialog's tab strip, nothing to configure,
nothing saved. Windows Terminal is a better local terminal than afterm is trying to be.

## Install

Download the latest MSI or setup `.exe` from
[Releases](https://github.com/afarzin1/afterm/releases) and run it. afterm runs on Windows 10 and 11;
the installer fetches the WebView2 runtime if it is missing.

The builds are not code-signed yet, so Windows SmartScreen will warn the first time you run the
installer: choose *More info ▸ Run anyway*. Every release is built from this repository by
[GitHub Actions](.github/workflows/build.yml), and you can build it yourself (below).

SSH sessions use afterm's own SSH implementation by default. For hosts whose configuration it
cannot honour faithfully it falls back to the Windows OpenSSH Client (`ssh.exe`) by itself, so it is
worth having that optional Windows feature installed.

### Updating

The interface is compiled into `afterm.exe`, so that one file is the whole app. To update without
reinstalling, run [`scripts/get-afterm.ps1`](scripts/get-afterm.ps1) in PowerShell:

```powershell
./scripts/get-afterm.ps1 -Install
```

It downloads `afterm.exe` from the latest release, closes afterm if it is running and replaces the
installed copy. Without `-Install` it just saves `afterm.exe` in the current directory. Run the
installer instead when the release notes mention the icon, shortcuts or installer itself.

## Features

- Multiple persistent SSH tabs, on afterm's own connection or through the OpenSSH client
- PowerShell and Command Prompt on this machine, one button each and no settings behind either
- Nested horizontal and vertical workspaces: `Ctrl Shift + Arrow` splits the active
  tab, `Alt Shift + Arrow` moves focus, and dividers resize by mouse or keyboard
- No outlines anywhere: workspaces that touch are told apart by shade, not by a line
- The chrome sizes itself to the display: the file pane, the tab strip and the status bar
  grow on a 1440p or 4K panel instead of staying at the size a 1080p one wants
- Tabs open at a comfortable width and give it back as more are opened, the way a browser's do
- JetBrains Mono, bundled with the app, on a single-surface theme: Stockholm, blue-tinted, or Carbon, cool grey
- Typed input highlighted against output, and the last command pinned while its output scrolls —
  from the shell itself where OSC 133 marks are installed, and inferred from keystrokes where they are not
- Unicode 11 character widths, so emoji and CJK occupy the same cells here as on the host
- `Ctrl Shift PgUp`/`PgDn` jump between commands, and `Ctrl Shift O` selects and copies what one
  printed — the last one, or the one jumped to — with no dragging through four hundred lines
- Quick select, `Ctrl Shift S`: every address, hash, container id, UUID, path, URL and process id on
  screen gets a letter, and typing it copies that — Shift-typing pastes it at the prompt as well
- Links open in the browser on `Ctrl` click, both URLs a program printed and OSC 8 hyperlinks
  (`ls --hyperlink`, compilers, Claude Code). The pointer shows where a link really goes first,
  because an OSC 8 link's text need not resemble its target, and only `http` and `https` are opened
- A saved host can keep its shell in tmux on the far end, so a dropped connection leaves the shell
  running and the reconnect lands back in it; closing the tab ends it. Off until *Settings ▸ Keep
  shells running in tmux* is switched on, and then opt in per host, because it hands the scrollback
  to tmux; a host without tmux gets its ordinary shell
- `Ctrl Shift Z` zooms one workspace to fill the window and gives the split back unchanged
- A tab marks itself when the coding agent in it — Claude Code, Codex, anything — finishes its
  turn, with nothing to set up on the host, and flashes the taskbar if afterm is behind something
- Real file browsing over the session's own SFTP channel: drag files in from Explorer to upload,
  drag one out to download, and rename, delete or make directories in place
- Images open where they are: double-click one, or its eye icon, and it is shown over the window
  with ← and → through the rest of the directory's images, and nothing is left in Downloads
- Whole folders in both directions — drag one in, or select one and download it. Links are never
  followed, nothing at either end is replaced without an answer, and the status says what arrived
- The file pane follows the shell with nothing to install: `cd` in the terminal and the listing
  moves with it, once per change, so a pane browsed somewhere else stays where it was put
- Real host telemetry from the far end: CPU, memory, disk, network and uptime
- Password, keyboard-interactive, public-key and agent authentication, chosen by what the host says it accepts
- Remembered passwords, off unless you ask per saved host: kept in the operating system's own
  credential store, bound to that host's account, address and host key, and never returned to the
  interface
- Clipboard where a Linux terminal keeps it: `Ctrl Shift C` copies, `Ctrl Shift V` and
  right-click paste, and `Ctrl C` interrupts every time — a stray selection can never swallow it
- Paste a screenshot into a coding agent on the host: with an image on the clipboard and no text,
  `Ctrl Shift V` in an SSH tab sends it to `~/.cache/afterm/paste` on the host and pastes its path,
  which Claude Code and its like take as an attached image
- Copy on select, on out of the box and switchable in *Settings*: highlight with the mouse and the
  text is on the clipboard when you let go, the way PuTTY does it, with a word in the corner saying
  so
- `Ctrl Shift F` finds text anywhere in the scrollback, with every hit marked and counted, and
  `PgUp`/`PgDn` page through it — except in a full-screen program, which gets the key itself
- `F11` puts the window full screen and takes it back, the way a browser does
- Saved hosts under names you choose, on the first tab of `Ctrl Shift N` with a number each,
  so a session is two keystrokes; plus a command palette and one-click
  `authorized_keys` installation of keys found on this machine
- Remote downloads go to Windows' configured Downloads known folder, including
  when it has been redirected, and never replace what is already there: a taken
  name gets a `(1)`, and nothing lands under its real name until it is whole

Nothing in the interface is placeholder data. A value afterm cannot measure renders as `—`, and a
listing it cannot read shows the error the backend returned, labelled with what kind of failure it
was: a refused credential offers a password prompt, an unreachable host offers a retry, and a host
afterm will not carry natively says so by name.

How each of these works, and the security properties afterm holds itself to — host keys checked
before any credential is offered, passwords never returned to the interface, links limited to
`http` and `https` — are in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Build from source

You need Node.js 20.19+ (or 22.12+), stable Rust, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```powershell
git clone https://github.com/afarzin1/afterm.git
cd afterm
npm install
npm run app            # run it, with live reload for the interface
npm run tauri build    # MSI and NSIS installers in src-tauri/target/release/bundle
```

## Contributing

Bug reports and pull requests are welcome. [`CONTRIBUTING.md`](CONTRIBUTING.md) has the
development loops, the test commands and how releases are built; [`ROADMAP.md`](ROADMAP.md) has
what is planned and what was ruled out.

## License

afterm is released under the [MIT license](LICENSE).

JetBrains Mono, bundled with the app, is by JetBrains under the SIL Open Font License 1.1; see
[`src/fonts/LICENSE-jetbrains-mono.txt`](src/fonts/LICENSE-jetbrains-mono.txt).
