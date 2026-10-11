#!/usr/bin/env bash
# Run the frontend checks against the Node the project actually requires.
#
# package.json declares node >=20.19 and Vite 8 enforces it, so on a machine
# with an older Node — which is every Ubuntu LTS — `npm test` and `npm run
# build` fail before they start, with a SyntaxError out of a bundler rather than
# anything to do with the change being tested. This runs them in the official
# Node image against the working tree, so nothing has to be installed on the
# host and nothing outside the repository is written.
#
# The Rust half of `npm run verify` is not here: cargo works on the host.
#
#   scripts/verify-in-container.sh          tsc, vitest and the production build
#   scripts/verify-in-container.sh shoot    the above, plus screenshots in ./shots
#
# `shoot` renders the built app in headless Chrome and writes two PNGs per
# colour scheme: the window, and the window with Settings open. The browser
# preview has no Rust backend, so what it captures is the chrome and the
# preview shell — which is enough to see a colour scheme, and the only way to
# see one without a Windows machine.
#
# Needs Docker. Nothing else.
set -euo pipefail

NODE_IMAGE=node:22-slim
CHROME_IMAGE=zenika/alpine-chrome:latest
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

# As the invoking user, or every file the build writes lands in the working
# tree owned by root.
run_node() {
  docker run --rm \
    --volume "$ROOT":/app --workdir /app \
    --user "$(id -u):$(id -g)" --env HOME=/tmp \
    "$NODE_IMAGE" "$@"
}

echo "==> types"
run_node npx tsc --noEmit

echo "==> tests"
run_node npx vitest run

echo "==> build"
run_node npm run build

[ "${1:-}" = "shoot" ] || exit 0

# Read out of theme.ts rather than listed here, so a third scheme is shot
# without touching this script.
SCHEMES=$(run_node node -e 'process.stdout.write(
  require("fs").readFileSync("src/theme.ts","utf8")
    .match(/^  id: "(\w+)",$/gm).map(l => l.match(/"(\w+)"/)[1]).join(" "))')
echo "==> screenshots: $SCHEMES"

STAGE=$(mktemp -d)
SERVERS=()
cleanup() {
  for pid in ${SERVERS[@]+"${SERVERS[@]}"}; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$STAGE"
}
trap cleanup EXIT INT TERM
mkdir -p shots

# True when something is accepting connections on the port.
listening() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

# One server per scheme, each rooted at its own copy of the build: the build
# asks for its assets from the site root, so a copy under a subdirectory would
# ask for /assets, get a 404, and screenshot as a blank white window.
#
# Both redirections belong to the background subshell rather than to python. A
# job that inherits this script's stdout holds the pipe open after the script
# itself is done, and anything reading it — `| tail`, a CI log collector —
# waits forever for an EOF that never comes.
serve() {
  ( cd "$1" && exec python3 -m http.server "$2" ) >/dev/null 2>&1 </dev/null &
  SERVERS+=($!)
}

# Virtual time, so the page's own timers run as fast as the CPU allows and the
# shot is of a settled interface rather than of whatever had painted by a
# wall-clock deadline.
shoot() {
  docker run --rm --network host \
    --volume "$ROOT/shots":/out --user "$(id -u):$(id -g)" \
    "$CHROME_IMAGE" --no-sandbox --headless --disable-gpu --hide-scrollbars \
    --virtual-time-budget=6000 --window-size=1280,780 \
    --screenshot="/out/$1.png" "$2" 2>/dev/null
  echo "    shots/$1.png"
}

PORT=8731
for scheme in $SCHEMES; do
  # A port a killed run left behind would still serve that run's build, and the
  # screenshot would silently be of it instead.
  while listening "$PORT"; do PORT=$((PORT + 1)); done

  cp -r dist "$STAGE/$scheme"
  # Runs before the module script, so the scheme is stored by the time afterm
  # reads it — the same value clicking it in Settings would have written. With
  # ?settings it also opens the panel, which is where the scheme is chosen and
  # is the half of the interface a window shot does not show.
  sed -i "s|<script type=\"module\"|<script>localStorage.setItem('afterm.theme','$scheme');if(location.search==='?settings')addEventListener('load',()=>setTimeout(()=>document.querySelector('.sidebar-footer button').click(),200))</script><script type=\"module\"|" \
    "$STAGE/$scheme/index.html"

  serve "$STAGE/$scheme" "$PORT"
  until listening "$PORT"; do sleep 0.2; done

  shoot "$scheme" "http://localhost:$PORT/"
  shoot "$scheme-settings" "http://localhost:$PORT/?settings"
  PORT=$((PORT + 1))
done
