#!/usr/bin/env bash
# Stands up a throwaway OpenSSH server and a throwaway HOME, and runs the
# native transport against them.
#
# The in-process battery cannot cover the seams that talk to the *installed*
# OpenSSH — `ssh -G`, `ssh-keygen -F`, and an exec channel against a real shell.
# This does. It touches nothing in the developer's real ~/.ssh: the config and
# known_hosts the client reads are ones this script wrote, under a HOME it made.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="${TMPDIR:-/tmp}/afterm-live-$$"
port=${AFTERM_LIVE_PORT:-2224}
trap 'kill "$(cat "$work/sshd.pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$work"' EXIT
mkdir -p "$work/data/sub" "$work/home/.ssh"

ssh-keygen -q -t ed25519 -f "$work/hostkey" -N ''
ssh-keygen -q -t ed25519 -f "$work/home/.ssh/id_ed25519" -N ''
# An RSA key as well. A current OpenSSH refuses the legacy ssh-rsa signature,
# so this only authenticates if the client negotiates rsa-sha2-*.
ssh-keygen -q -t rsa -b 2048 -f "$work/home/.ssh/id_rsa" -N ''
# A second host key, never served, so a mismatch can be tested.
ssh-keygen -q -t ed25519 -f "$work/otherhost" -N ''
cat "$work/home/.ssh/id_ed25519.pub" "$work/home/.ssh/id_rsa.pub" > "$work/authorized_keys"
chmod 700 "$work/home/.ssh"
chmod 600 "$work/authorized_keys" "$work/hostkey" "$work/home/.ssh/id_ed25519" "$work/home/.ssh/id_rsa"

printf 'hello\n' > "$work/data/afile.txt"
printf 'x\n'     > "$work/data/file with space.txt"

sftp_server=$(command -v sftp-server || echo /usr/lib/openssh/sftp-server)
cat > "$work/sshd_config" <<CONF
Port $port
ListenAddress 127.0.0.1
HostKey $work/hostkey
AuthorizedKeysFile $work/authorized_keys
PidFile $work/sshd.pid
StrictModes no
UsePAM no
PasswordAuthentication no
Subsystem sftp $sftp_server
CONF

"$(command -v sshd || echo /usr/sbin/sshd)" -f "$work/sshd_config"
sleep 1

# The throwaway config the client is pointed at with -F. It cannot be found by
# setting HOME: OpenSSH resolves ~ through the password database, not $HOME.
cat > "$work/home/.ssh/config" <<CONF
Host afterm-live
    HostName 127.0.0.1
    Port $port
    User $(id -un)
    IdentityFile $work/home/.ssh/id_ed25519
    IdentitiesOnly yes
    UserKnownHostsFile $work/home/.ssh/known_hosts
    GlobalKnownHostsFile /dev/null

Host afterm-live-rsa
    HostName 127.0.0.1
    Port $port
    User $(id -un)
    IdentityFile $work/home/.ssh/id_rsa
    IdentitiesOnly yes
    UserKnownHostsFile $work/home/.ssh/known_hosts
    GlobalKnownHostsFile /dev/null
CONF
ssh-keyscan -p "$port" -t ed25519 127.0.0.1 > "$work/home/.ssh/known_hosts" 2>/dev/null
# The same host recorded with a key it does not serve: this must be refused.
sed "s|ssh-ed25519 .*|$(cut -d' ' -f1,2 "$work/otherhost.pub" | cut -d' ' -f2- | sed 's|^|ssh-ed25519 |')|" \
  "$work/home/.ssh/known_hosts" > "$work/home/.ssh/known_hosts_wrong"

echo "sshd on 127.0.0.1:$port, throwaway ssh_config at $work/home/.ssh/config"
echo

AFTERM_LIVE_CONFIG="$work/home/.ssh/config" \
AFTERM_LIVE_WRONG_KNOWN_HOSTS="$work/home/.ssh/known_hosts_wrong" \
AFTERM_LIVE_HOST=afterm-live \
AFTERM_LIVE_USER="$(id -un)" \
AFTERM_LIVE_PORT="$port" \
AFTERM_LIVE_DIR="$work/data" \
  cargo test --manifest-path "$here/../src-tauri/Cargo.toml" \
    --lib native::live -- --ignored --nocapture --test-threads=1
