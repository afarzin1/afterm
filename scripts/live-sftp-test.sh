#!/usr/bin/env bash
# End-to-end SFTP test against a throwaway OpenSSH server.
#
# Starts an sshd on a high port with its own host key and authorized_keys, points
# $HOME at a fixture so the ssh client picks up the matching identity, then runs
# the ignored `talks_to_a_real_sftp_server` test against it. Touches nothing in
# the developer's real ~/.ssh.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="${TMPDIR:-/tmp}/afterm-sftp-test-$$"
trap 'kill "$(cat "$work/sshd.pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$work"' EXIT
mkdir -p "$work/home/.ssh" "$work/data/sub"

ssh-keygen -q -t ed25519 -f "$work/hostkey" -N ''
ssh-keygen -q -t ed25519 -f "$work/home/.ssh/id_ed25519" -N ''
cp "$work/home/.ssh/id_ed25519.pub" "$work/authorized_keys"
chmod 600 "$work/authorized_keys" "$work/hostkey"

printf 'hello\n' > "$work/data/afile.txt"
printf 'x\n'     > "$work/data/file with space.txt"
printf 'deeper\n' > "$work/data/sub/deep.txt"
ln -sf afile.txt "$work/data/alink"

sftp_server=$(command -v sftp-server || echo /usr/lib/openssh/sftp-server)
cat > "$work/sshd_config" <<CONF
Port 2222
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

# OpenSSH reads the passwd entry rather than $HOME, so the fixture identity and
# known_hosts are handed to the client explicitly instead of through a config
# file. Nothing in the developer's real ~/.ssh is read or written.
ssh-keyscan -p 2222 -t ed25519 127.0.0.1 > "$work/known_hosts" 2>/dev/null

AFTERM_TEST_HOST=127.0.0.1 \
AFTERM_TEST_PORT=2222 \
AFTERM_TEST_USER="$(id -un)" \
AFTERM_TEST_DIR="$work/data" \
AFTERM_TEST_KEY="$work/home/.ssh/id_ed25519" \
AFTERM_TEST_KNOWN_HOSTS="$work/known_hosts" \
  cargo test --manifest-path "$root/src-tauri/Cargo.toml" \
    -- --ignored --nocapture talks_to_a_real_sftp_server
