#!/usr/bin/env bash
# Stands up a throwaway OpenSSH server and runs the native-SSH spike against it.
# Touches nothing in the developer's real ~/.ssh.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="${TMPDIR:-/tmp}/spike-sshd-$$"
trap 'kill "$(cat "$work/sshd.pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$work"' EXIT
mkdir -p "$work/data/sub"

ssh-keygen -q -t ed25519 -f "$work/hostkey" -N ''
ssh-keygen -q -t ed25519 -f "$work/clientkey" -N ''
# A second host key, never served, used to build a known_hosts that MISMATCHES.
ssh-keygen -q -t ed25519 -f "$work/otherhost" -N ''
cp "$work/clientkey.pub" "$work/authorized_keys"
chmod 600 "$work/authorized_keys" "$work/hostkey" "$work/clientkey"

printf 'hello\n' > "$work/data/afile.txt"
printf 'x\n'     > "$work/data/file with space.txt"

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

# The real key, as the client will see it.
ssh-keyscan -p 2222 -t ed25519 127.0.0.1 > "$work/known_hosts" 2>/dev/null
# The same host, recorded with the wrong key: this must be refused.
sed "s|ssh-ed25519 .*|$(cut -d' ' -f1,2 "$work/otherhost.pub")|" \
  "$work/known_hosts" > "$work/bad_known_hosts"

echo "host key served : $(cut -c1-50 "$work/hostkey.pub")…"
echo "known_hosts     : $(cut -c1-50 "$work/known_hosts")…"
echo "bad known_hosts : $(cut -c1-50 "$work/bad_known_hosts")…"
echo

SPIKE_HOST=127.0.0.1 \
SPIKE_PORT=2222 \
SPIKE_USER="$(id -un)" \
SPIKE_KEY="$work/clientkey" \
SPIKE_KNOWN_HOSTS="$work/known_hosts" \
SPIKE_BAD_KNOWN_HOSTS="$work/bad_known_hosts" \
SPIKE_DIR="$work/data" \
  cargo run --quiet --manifest-path "$here/Cargo.toml"
