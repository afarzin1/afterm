# Contributing to afterm

Issues and pull requests are welcome at
[github.com/afarzin1/afterm](https://github.com/afarzin1/afterm). This file covers building afterm,
the loops to develop against, and how the Windows build works. For how afterm is put together, see
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Where things are written down

- [`ROADMAP.md`](ROADMAP.md): what is left, what was rejected and why, and the known limits.
- [`CHANGELOG.md`](CHANGELOG.md): what is built, and why it was built that way.
- [`OPEN-ISSUE.md`](OPEN-ISSUE.md): the code reviews and the decisions they turned on.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): the design, and the security properties afterm
  holds itself to.

## Prerequisites

- Node.js 20.19 or newer, or 22.12 or newer. Some build dependencies refuse anything between them,
  including 22.11.
- Rust (stable, via [rustup](https://rustup.rs)).
- The [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform.

```powershell
git clone https://github.com/afarzin1/afterm.git
cd afterm
npm install
npm run app
```

## Before sending a pull request

Run `npm run verify` — types, `cargo check`, and both test suites — and add a test that fails
without your change where one can be written. Keep each pull request to one change, and say in its
description what it fixes and how you checked it, especially for anything Windows-specific.

## Iterating quickly

The GitHub Actions build makes installers and takes minutes. It runs by hand or on a release tag,
and it is not the loop to develop against. In order of speed:

| Loop | Command | Rebuilds |
| --- | --- | --- |
| Interface only | `npm run dev` | Instant HMR, no Rust. Opens a preview shell with no PTY. |
| Whole app | `npm run app` | Instant HMR for the interface; only Rust changes recompile. |
| Checks | `npm run verify` | Types, lint-level `cargo check`, and the interface and backend test suites. |
| Unbundled build | `npm run build:fast` | A real binary with no installer packaging. |

`npm run app` is the one to leave running. The webview loads Vite, so every interface change is
live immediately; only edits under `src-tauri/` trigger a Rust rebuild, and the tuned `dev` profile
in `src-tauri/Cargo.toml` keeps those to a few seconds by dropping full debug info and optimising
dependencies once.

Additional checks:

```powershell
npm run check:windows   # type-check the Windows target from any platform
npm run test:rust       # backend unit tests
npm run test:sftp       # end-to-end SFTP through the OpenSSH client (needs an sshd)
npm run test:native     # the native transport end to end against a throwaway sshd
```

`npm run check:windows` works on Linux and macOS too — it compiles the real Windows code path
without needing Windows, which catches backend mistakes long before the pipeline does. It needs
more than it used to: the SSH crypto backend is C and assembly, so cross-compiling now wants an
MSVC-compatible C compiler and the Windows CRT headers. `cargo-xwin` fetches the headers and
`clang-cl` does the compiling:

```bash
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin
sudo apt install clang llvm   # clang-cl for the C, llvm-rc for the build script
```

Without those the check stops inside `aws-lc-sys` rather than in afterm's own code, which is a
toolchain problem and not a Windows one. The GitHub Windows runner has MSVC and needs none of this; the
`prebuilt-nasm` feature in `src-tauri/Cargo.toml` is what keeps it from needing NASM as well.

### When the host's Node is too old

Vite 8 refuses anything below Node 20.19, so on a machine with an older one — which is every Ubuntu
LTS — `npm test` and `npm run build` fail before they start, with a `SyntaxError` out of a bundler
rather than anything to do with the change being tested. `scripts/verify-in-container.sh` runs both
in the official Node image against the working tree, so nothing has to be installed on the host and
nothing outside the repository is written:

```bash
scripts/verify-in-container.sh         # tsc, vitest and the production build
scripts/verify-in-container.sh shoot   # the above, plus screenshots in ./shots
```

Only the frontend half of `npm run verify` is in there; `cargo check` and `cargo test` run on the
host on any platform.

`shoot` renders the built app in headless Chrome and writes two PNGs per colour scheme — the window,
and the window with Settings open. The browser preview has no Rust backend, so what it captures is
the chrome and the preview shell, which is enough to see a colour scheme and is the only way to see
one without a Windows machine. It needs Docker and reads the scheme list out of `src/theme.ts`, so a
third scheme is shot without touching the script.

### The icon

`app-icon.png` at the repository root is the master, and everything in
`src-tauri/icons/` is generated from it by Tauri's own tool:

```bash
npm run tauri icon        # reads ./app-icon.png, writes src-tauri/icons/
```

Windows builds use `icons/icon.ico`, which is what `tauri.conf.json` names and
what ends up on `afterm.exe` and both installers. The rest of the directory is
iOS, Android and macOS sizes that afterm does not ship but keeps in step, so a
new master does not leave half the set behind.

The master is 1024×1024, which is what the tool wants: every size it writes is
a downscale, so nothing in the set is an upscale of anything. Replacing the
icon means replacing that one file and running the command above.

The master includes the rounded desktop tile and proportionate space around the
`af>` mark. Keep that padding in the source: Tauri downsizes the complete tile
for each platform rather than adding or removing margins at individual sizes.

See `ROADMAP.md` for what is left, what was rejected and why, and the known
limits of the current milestone; `CHANGELOG.md` for what is built and why it
was built that way.

## Build installers

```powershell
npm ci
npm run tauri build
```

Tauri writes MSI and per-user NSIS installers beneath `src-tauri/target/release/bundle`.

Neither the executable nor the installers are signed. Windows code signing needs a certificate
issued to a validated organisation, and since June 2023 every new one has to live on FIPS-140-2
hardware or a cloud signing service — so there is no self-signed route that SmartScreen or Chrome
will respect, and downloads draw a warning until there is one.

## The Windows build on GitHub Actions

[`.github/workflows/build.yml`](.github/workflows/build.yml) builds on a GitHub-hosted Windows
Server 2025 runner: the frontend tests and type check, the backend tests in the release profile,
then `afterm.exe` and both installers, published as the `afterm-exe` and `afterm-windows`
artifacts. Run it from the **Actions** tab (*Build ▸ Run workflow*) or with `gh`:

```bash
gh workflow run build.yml                                                # everything
gh workflow run build.yml -f build_installers=false -f run_tests=false   # the fast lane
gh workflow run build.yml -f release=v1.0.1                              # publish a release
```

Unticking *Build MSI and NSIS installers* links the executable with `--no-bundle` and skips
packaging, which is enough to test a change: the frontend is compiled into `afterm.exe`, so that one
file is the whole app and replacing it updates an existing installation. Unticking *Run the native
backend tests* as well removes the test binary's own link. Both are on by default, because a run
that publishes an installer should have been tested.

**Releases.** Either fill in *Release tag* (e.g. `v1.0.1`) when running the workflow, or push that
tag. Both run the full build and publish `afterm.exe` and both installers as a GitHub Release; a
manual run creates the tag at the commit it built. Bump the version in `package.json`,
`src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` first: the run refuses a tag that does not
match `tauri.conf.json`, so the installers always carry the version on their release.

**Signing.** The executable is built on its own, signed, and only then packed into the installers,
which are signed in turn — so the installed `afterm.exe` is signed, not just the setup program.
Signing goes through [SignPath Foundation](https://signpath.org) and switches on when the repository
has the SignPath settings described in [`.signpath/README.md`](.signpath/README.md); without them
the build is unsigned. Each signed release sends two signing requests, the executable and the
installers, and the run waits while an approver approves each in SignPath.

**Caching.** The cargo registry and `src-tauri/target` are cached on `Cargo.lock` and `Cargo.toml`,
not on the sources: the manifest is in the key because a profile change invalidates every artifact
without touching the lock file, and the sources are not, because a key that changed on every commit
would never hit. The first run after a dependency or profile change recompiles every crate and
saves a large cache; the run after it is fast again. Restoring takes seconds, so a warm run is
compile and link almost entirely — which is also why `[profile.release]` sets `lto = false`: thin
LTO was measured at four extra minutes a build for an executable 2% larger.

Installers from an earlier version can come back with a restored `target` directory, because WiX
and NSIS never clean their output, so the workflow deletes `target/release/bundle` before packaging.
Packaging is not incremental, so that costs nothing.
