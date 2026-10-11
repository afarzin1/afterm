# Native SSH spike

Throwaway code that established whether afterm can speak SSH itself instead of
driving `ssh.exe`. It is deliberately **not** part of the application build —
separate crate, not a workspace member, so `cargo test` in `src-tauri` ignores
it. The production native transport now lives in the application; this spike is
kept as the record of the API work that started v2, because re-deriving it by
trial and error would waste a day.

Run it:

```bash
bash run.sh                 # against a throwaway OpenSSH server (key auth)
cargo run --bin password    # password + keyboard-interactive, in-process server
cargo run --bin inproc      # the security battery, no external server at all
cargo run --bin sshconfig   # what an ssh_config parser does and does not expose
```

See `../../CHANGELOG.md` for what each one proved.
