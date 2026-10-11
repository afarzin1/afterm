use crate::metrics::Metrics;
use crate::transport::{classify_openssh, Failure};
use crate::winproc::hide_console;
use std::{
    io::{BufReader, Read},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Emitted by the remote shell once per interval. Kept deliberately small: four
/// short reads out of /proc rather than whole files, because this streams for
/// the life of the session.
pub const PROBE: &str = concat!(
    // `$PPID` is the sshd for this connection, captured once. Every channel on
    // the connection is one of its children, so the interactive shell is a
    // sibling of this probe and needs no PID passed in from outside.
    "P=$PPID; while :; do echo '===afterm===';",
    " head -1 /proc/uptime;",
    " grep -E '^(MemTotal|MemAvailable):' /proc/meminfo;",
    " head -1 /proc/stat;",
    " tail -n +3 /proc/net/dev;",
    // Disk comes from the same stream rather than a separate connection: the
    // earlier design asked sftp for it on a timer, which spawned a client per
    // poll and could saturate the thread pool.
    " df -kP / | tail -1;",
    // Where the shell is, for the file pane to follow.
    //
    // The sibling with a terminal is the shell. `tpgid` is the process group in
    // the foreground of that terminal, which is the shell itself at a prompt
    // and the nested one after `bash` or `sudo -i`; reading the sibling
    // directly answers with the outer shell's directory and goes stale the
    // moment anyone nests.
    //
    // The kernel's own child list rather than a sweep of /proc: a sweep stats
    // every process and cost 530ms a sample on a 195-process host, which this
    // loop would pay every two seconds for the life of the session. Two reads
    // instead. POSIX only — the shell running this may be dash or busybox, so
    // no `$(< file)` and no arrays.
    " for c in $(cat /proc/$P/task/$P/children 2>/dev/null); do",
    " s=$(cat /proc/$c/stat 2>/dev/null) || continue;",
    // Fields after the last `)`, because a process name may contain spaces or
    // parentheses and would otherwise shift every field after it:
    // 1 state, 2 ppid, 3 pgrp, 4 session, 5 tty_nr, 6 tpgid.
    " r=${s##*\\) }; set -- $r;",
    " [ \"$5\" = 0 ] && continue;",
    // An unreadable cwd is a shell afterm is not allowed to look at — `sudo -i`
    // leaves a root-owned process — and answers with nothing rather than an
    // error, which leaves the pane where the user had it.
    " d=$(readlink /proc/$6/cwd 2>/dev/null);",
    " [ -n \"$d\" ] && echo \"afterm-cwd $d\";",
    " break; done;",
    " sleep 2; done"
);

/// What separates one sample from the next. The same text `PROBE` echoes —
/// `concat!` takes literals only, so the two are spelled out separately and
/// have to stay in step.
const DELIMITER: &str = "===afterm===";

/// The most one line of the stream may take up, the most a whole block may, and
/// the most rows a block may hold, before it stops being telemetry and starts
/// being a way to fill this process's memory.
///
/// All three are far above anything a real host produces: the longest
/// legitimate line is a `/proc/net/dev` row of a few hundred bytes, and a block
/// is five lines plus one per network interface. A host with a thousand
/// interfaces still fits inside all three with room to spare.
///
/// `MAX_ROWS` exists because the byte limit alone did not close the hole it was
/// written for. A block's cost was measured as the sum of its line *contents*,
/// so a stream of nothing but newlines was accounted at zero bytes while every
/// empty line was retained: 100,000 of them were accepted, charged nothing, and
/// cost at least 3 MB of row storage. A `String` is three words before it holds
/// a character, so the count of rows is as much a resource as their length —
/// and the byte charge now includes the newline that ended each one, so an
/// empty line is never free either.
const MAX_LINE: usize = 8 * 1024;
const MAX_BLOCK: usize = 256 * 1024;
const MAX_ROWS: usize = 8 * 1024;

/// The most of a helper's diagnostic output afterm will hold.
///
/// stderr is read to classify a failure, which takes the first few lines; the
/// rest is a courtesy. `read_to_string` on a pipe the far end controls is not,
/// so the capture stops accumulating at this point and goes on draining, which
/// is the half that matters: a producer blocked on a full pipe is a helper that
/// never exits and a `wait` that never returns.
pub const MAX_DIAGNOSTIC: usize = 64 * 1024;

/// How long to wait for the process to be reaped after its output ends, before
/// reporting the exit without a status. Polled rather than blocked on, so
/// `Drop` can always take the lock and kill the child.
const REAP_TIMEOUT: Duration = Duration::from_secs(2);

/// Reassembles the helper's byte stream into the blocks `parse_block` reads,
/// and refuses to grow without bound while doing it.
///
/// Everything between two delimiters is held here until the second one
/// arrives, and the stream comes from the host. An authenticated host that has
/// been taken over — or one whose shell is simply broken — can withhold both
/// the newline and the delimiter for as long as it likes, and an unbounded
/// buffer turns that into an out-of-memory on the machine afterm is running
/// on. Both transports reassemble through this, so both stop in the same place
/// for the same reason.
pub struct Blocks {
    /// Bytes of the line in progress, converted only once it is whole so that
    /// a character split across two chunks is still one character.
    line: Vec<u8>,
    block: Vec<String>,
    held: usize,
}

/// Read everything `source` produces, keeping at most `limit` bytes of it.
///
/// The reading does not stop at the limit — the rest is read and dropped. That
/// is deliberate: the producer is a child process writing to a pipe, and a pipe
/// nobody drains fills, blocks the writer, and leaves a process that cannot
/// exit. Bounding the memory and bounding the pipe are different problems and
/// only one of them is solved by stopping.
///
/// A capture that hit the limit says so, so nothing downstream mistakes a
/// truncated answer for a complete one.
pub fn read_capped(source: &mut impl Read, limit: usize) -> (String, bool) {
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = match source.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        let room = limit.saturating_sub(kept.len());
        if room == 0 {
            truncated = true;
            continue;
        }
        let take = room.min(read);
        kept.extend_from_slice(&chunk[..take]);
        truncated |= take < read;
    }
    (String::from_utf8_lossy(&kept).into_owned(), truncated)
}

impl Blocks {
    pub fn new() -> Self {
        Self { line: Vec::new(), block: Vec::new(), held: 0 }
    }

    /// Feed one chunk of the stream, calling `complete` with every block it
    /// finishes.
    ///
    /// `Err` is a limit passed: whatever is on the other end is not telemetry,
    /// and the caller is expected to stop reading it rather than carry on.
    pub fn feed(
        &mut self,
        chunk: &[u8],
        mut complete: impl FnMut(&[String]),
    ) -> Result<(), &'static str> {
        for byte in chunk {
            if *byte != b'\n' {
                self.line.push(*byte);
                if self.line.len() > MAX_LINE {
                    return Err("this host sent a telemetry line that never ended");
                }
                continue;
            }
            let line = String::from_utf8_lossy(&self.line).trim_end().to_string();
            self.line.clear();
            if line.trim() == DELIMITER {
                if !self.block.is_empty() {
                    complete(&self.block);
                    self.block.clear();
                    self.held = 0;
                }
                continue;
            }
            // The newline is charged too, so a line with nothing on it still
            // costs something: an empty row is a `String` this process has to
            // hold, whatever it contains.
            self.held += line.len() + 1;
            self.block.push(line);
            if self.held > MAX_BLOCK || self.block.len() > MAX_ROWS {
                return Err("this host sent a telemetry block that never ended");
            }
        }
        Ok(())
    }
}

#[derive(Default, Clone, Copy)]
pub struct CpuSample {
    busy: u64,
    total: u64,
}

#[derive(Default, Clone, Copy)]
pub struct NetSample {
    received: u64,
    transmitted: u64,
}

/// Where a telemetry helper has got to.
///
/// v1 had no such thing. `connect` returned `Ok` as soon as the process
/// spawned, discarded its stderr, and left the dead handle in the session slot,
/// so a host that refused the helper's credentials reported `no data yet`
/// forever and was never retried. The `needs key auth` message the README
/// described could not be reached.
#[derive(Debug, Clone)]
pub enum HelperState {
    /// Spawned; no sample has arrived yet.
    Connecting,
    /// At least one sample has been parsed.
    Ready,
    /// The process ended. The reason is whatever it wrote before it did.
    Exited(Failure),
}

/// A long-lived `ssh` process that streams host telemetry for one SSH session.
///
/// One connection is opened for the life of the session rather than one per
/// poll, so the remote sees a single login and the status bar costs almost
/// nothing to keep current.
pub struct RemoteMetrics {
    latest: Arc<Mutex<Option<Metrics>>>,
    state: Arc<Mutex<HelperState>>,
    child: Arc<Mutex<Child>>,
}

impl RemoteMetrics {
    pub fn connect(host: &str, user: &str, port: u16) -> Result<Self, Failure> {
        let mut builder = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
        let mut child = hide_console(&mut builder)
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-p",
                &port.to_string(),
                &format!("{user}@{host}"),
                PROBE,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Piped, not discarded: this is the only place the host says why it
            // refused, and both the status bar and the transport selector need
            // the difference between "wrong credentials" and "no route".
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Failure::spawn(if cfg!(windows) { "ssh.exe" } else { "ssh" }, &error))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Failure::internal("ssh stdout unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| Failure::internal("ssh stderr unavailable"))?;
        let latest: Arc<Mutex<Option<Metrics>>> = Arc::new(Mutex::new(None));
        let state = Arc::new(Mutex::new(HelperState::Connecting));
        let child = Arc::new(Mutex::new(child));

        // Drain stderr on its own thread, keeping a bounded amount of it. A
        // full pipe would otherwise block the client; an unbounded capture
        // would let it fill this process instead.
        let diagnostics: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&diagnostics);
        std::thread::spawn(move || {
            let (text, _) = read_capped(&mut stderr, MAX_DIAGNOSTIC);
            *sink.lock().unwrap() = text;
        });

        let samples = Arc::clone(&latest);
        let lifecycle = Arc::clone(&state);
        let reap = Arc::clone(&child);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut chunk = [0_u8; 8 * 1024];
            let mut blocks = Blocks::new();
            let mut previous_cpu: Option<CpuSample> = None;
            let mut previous_net: Option<(NetSample, Instant)> = None;
            // Set when the stream was stopped for sending more than afterm will
            // hold. That is the reason for the exit, and it outranks whatever
            // the client happened to write to stderr.
            let mut refused: Option<&'static str> = None;

            loop {
                let read = match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let outcome = blocks.feed(&chunk[..read], |block| {
                    let metrics = parse_block(block, &mut previous_cpu, &mut previous_net);
                    *samples.lock().unwrap() = Some(metrics);
                    *lifecycle.lock().unwrap() = HelperState::Ready;
                });
                if let Err(reason) = outcome {
                    refused = Some(reason);
                    // Nothing is going to read the rest of it, and a host that
                    // is flooding the pipe should be told to stop by having the
                    // client taken away from it.
                    let _ = reap.lock().unwrap().kill();
                    break;
                }
            }

            // The stream ended, so the process is on its way out. Stop reporting
            // stale numbers, then work out why it went.
            *samples.lock().unwrap() = None;
            let deadline = Instant::now() + REAP_TIMEOUT;
            loop {
                // Take the lock only for the check: `Drop` needs it to kill.
                let status = reap.lock().unwrap().try_wait();
                match status {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if Instant::now() >= deadline => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
            let text = diagnostics.lock().unwrap().clone();
            *lifecycle.lock().unwrap() = HelperState::Exited(match refused {
                Some(reason) => Failure::operation(reason),
                None => classify_openssh(&text),
            });
        });

        Ok(Self { latest, state, child })
    }

    /// The most recent sample, or `None` until the first one lands.
    pub fn sample(&self) -> Option<Metrics> {
        self.latest.lock().unwrap().clone()
    }

    /// Where the helper has got to, so a dead one can be reported and replaced
    /// rather than sitting in the session slot pretending to be alive.
    pub fn state(&self) -> HelperState {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for RemoteMetrics {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

pub fn parse_block(
    block: &[String],
    previous_cpu: &mut Option<CpuSample>,
    previous_net: &mut Option<(NetSample, Instant)>,
) -> Metrics {
    let mut metrics = Metrics::default();
    let mut memory_total = None;
    let mut memory_available = None;
    let mut net = NetSample::default();

    for line in block {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("afterm-cwd ") {
            // Ahead of the interface branch below rather than beside it: a
            // directory may contain a colon, and that branch reads anything
            // with one as a row of /proc/net/dev.
            metrics.directory = Some(rest.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("MemTotal:") {
            memory_total = first_number(rest).map(|value| value * 1024);
        } else if let Some(rest) = trimmed.strip_prefix("MemAvailable:") {
            memory_available = first_number(rest).map(|value| value * 1024);
        } else if let Some(rest) = trimmed.strip_prefix("cpu ") {
            let values: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect();
            if values.len() >= 5 {
                let total: u64 = values.iter().sum();
                // idle + iowait are the non-working jiffies.
                let idle = values[3] + values[4];
                let sample = CpuSample {
                    busy: total.saturating_sub(idle),
                    total,
                };
                if let Some(last) = *previous_cpu {
                    let total_delta = sample.total.saturating_sub(last.total);
                    let busy_delta = sample.busy.saturating_sub(last.busy);
                    if total_delta > 0 {
                        metrics.cpu = Some(busy_delta as f32 / total_delta as f32 * 100.0);
                    }
                }
                *previous_cpu = Some(sample);
            }
        } else if let Some((interface, counters)) = trimmed.split_once(':') {
            // Loopback traffic is not network traffic.
            if interface.trim() == "lo" {
                continue;
            }
            let values: Vec<u64> = counters
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect();
            if values.len() >= 9 {
                net.received += values[0];
                net.transmitted += values[8];
            }
        } else if let Some(rest) = trimmed.strip_prefix("/dev/") {
            // df: filesystem, 1K-blocks, used, available, capacity, mount.
            let values: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect();
            if values.len() >= 3 {
                metrics.disk_total = Some(values[0] * 1024);
                metrics.disk_used = Some(values[1] * 1024);
            }
        } else if metrics.uptime.is_none() {
            // /proc/uptime is the only bare "float float" line in the block.
            if let Some(seconds) = trimmed.split_whitespace().next() {
                if let Ok(value) = seconds.parse::<f64>() {
                    metrics.uptime = Some(value as u64);
                }
            }
        }
    }

    metrics.total_memory = memory_total;
    metrics.used_memory = match (memory_total, memory_available) {
        (Some(total), Some(available)) => Some(total.saturating_sub(available)),
        _ => None,
    };

    let now = Instant::now();
    if let Some((last, at)) = *previous_net {
        let seconds = now.duration_since(at).as_secs_f64();
        if seconds > 0.0 {
            metrics.download_rate =
                Some(net.received.saturating_sub(last.received) as f64 / seconds);
            metrics.upload_rate =
                Some(net.transmitted.saturating_sub(last.transmitted) as f64 / seconds);
        }
    }
    *previous_net = Some((net, now));

    metrics
}

fn first_number(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn needs_two_samples_before_reporting_cpu_or_network() {
        let block = lines(
            "12345.67 98765.43\nMemTotal:        8000000 kB\nMemAvailable:    2000000 kB\n\
             cpu  100 0 50 800 50 0 0 0 0 0\n  eth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0",
        );
        let (mut cpu, mut net) = (None, None);

        // First block establishes the baseline: rates are unknowable.
        let first = parse_block(&block, &mut cpu, &mut net);
        assert_eq!(first.cpu, None);
        assert_eq!(first.download_rate, None);
        // Absolute values are available immediately.
        assert_eq!(first.total_memory, Some(8_000_000 * 1024));
        assert_eq!(first.used_memory, Some(6_000_000 * 1024));
        assert_eq!(first.uptime, Some(12345));

        // Second block: busy jiffies +100, total +200 => 50%.
        let next = lines(
            "12347.67 98767.43\nMemTotal:        8000000 kB\nMemAvailable:    2000000 kB\n\
             cpu  200 0 100 900 100 0 0 0 0 0\n  eth0: 3000 0 0 0 0 0 0 0 4000 0 0 0 0 0 0 0",
        );
        let second = parse_block(&next, &mut cpu, &mut net);
        assert_eq!(second.cpu, Some(50.0));
        assert!(second.download_rate.unwrap() > 0.0);
    }

    #[test]
    fn reads_disk_usage_from_the_same_stream() {
        let block = lines(
            "1.0 1.0\ncpu  1 0 1 1 1 0 0 0 0 0\n\
             /dev/mapper/vg-lv  62599772  37810448  21575148  64% /",
        );
        let (mut cpu, mut net) = (None, None);
        let metrics = parse_block(&block, &mut cpu, &mut net);
        assert_eq!(metrics.disk_total, Some(62_599_772 * 1024));
        assert_eq!(metrics.disk_used, Some(37_810_448 * 1024));
    }

    /// A directory is a path, and a path may contain a colon. The interface
    /// branch reads anything with one as a row of /proc/net/dev, so the order
    /// of the two is what keeps `/srv/a:b` a directory — this fails if the cwd
    /// branch is ever moved below it.
    #[test]
    fn a_directory_containing_a_colon_is_not_read_as_an_interface() {
        let block = lines(
            "1.0 1.0\ncpu  1 0 1 1 1 0 0 0 0 0\n\
             afterm-cwd /srv/a:b\n\
               eth0: 500 0 0 0 0 0 0 0 700 0 0 0 0 0 0 0",
        );
        let (mut cpu, mut net) = (None, None);
        let metrics = parse_block(&block, &mut cpu, &mut net);
        assert_eq!(metrics.directory.as_deref(), Some("/srv/a:b"));
        let (sample, _) = net.unwrap();
        assert_eq!(sample.received, 500);
        assert_eq!(sample.transmitted, 700);
    }

    /// A host with no /proc, or a shell afterm may not look at, reports no
    /// directory at all rather than an empty one the pane would try to list.
    #[test]
    fn a_block_without_a_directory_reports_none() {
        let block = lines("1.0 1.0\ncpu  1 0 1 1 1 0 0 0 0 0");
        let (mut cpu, mut net) = (None, None);
        assert_eq!(parse_block(&block, &mut cpu, &mut net).directory, None);
    }

    #[test]
    fn excludes_loopback_from_network_totals() {
        let block = lines(
            "1.0 1.0\ncpu  1 0 1 1 1 0 0 0 0 0\n\
                 lo: 999999 0 0 0 0 0 0 0 999999 0 0 0 0 0 0 0\n\
               eth0: 500 0 0 0 0 0 0 0 700 0 0 0 0 0 0 0",
        );
        let (mut cpu, mut net) = (None, None);
        parse_block(&block, &mut cpu, &mut net);
        let (sample, _) = net.unwrap();
        assert_eq!(sample.received, 500);
        assert_eq!(sample.transmitted, 700);
    }

    /// The stream arrives in whatever pieces the connection hands over, and a
    /// block is only a block once its delimiter has.
    #[test]
    fn reassembles_blocks_out_of_the_pieces_the_stream_arrives_in() {
        let mut blocks = Blocks::new();
        let mut seen: Vec<Vec<String>> = Vec::new();
        let feed = |blocks: &mut Blocks, chunk: &str, seen: &mut Vec<Vec<String>>| {
            blocks.feed(chunk.as_bytes(), |block| seen.push(block.to_vec())).unwrap();
        };

        feed(&mut blocks, "===afterm===\n1.0 1.0\nMemTo", &mut seen);
        assert!(seen.is_empty(), "nothing is complete until the next delimiter");
        feed(&mut blocks, "tal:  8 kB\n===afterm===\n2.0 2.0\n", &mut seen);
        assert_eq!(seen, vec![vec!["1.0 1.0".to_string(), "MemTotal:  8 kB".to_string()]]);

        // A CRLF host, and a delimiter split across two chunks.
        seen.clear();
        feed(&mut blocks, "===aft", &mut seen);
        feed(&mut blocks, "erm===\r\n", &mut seen);
        assert_eq!(seen, vec![vec!["2.0 2.0".to_string()]]);
    }

    /// A host that never sends the newline, or never sends the delimiter, is
    /// asking afterm to hold everything it says until it stops. Neither buffer
    /// is allowed to grow without end, and passing a limit ends the stream
    /// rather than trimming it: what is on the other end is not telemetry.
    #[test]
    fn refuses_a_line_or_a_block_that_never_ends() {
        let mut blocks = Blocks::new();
        let flood = "x".repeat(MAX_LINE + 1);
        assert!(blocks.feed(flood.as_bytes(), |_| unreachable!()).is_err());

        let mut blocks = Blocks::new();
        let line = format!("{}\n", "y".repeat(1024));
        let outcome = loop {
            if let Err(reason) = blocks.feed(line.as_bytes(), |_| unreachable!()) {
                break reason;
            }
        };
        assert!(outcome.contains("block"), "{outcome}");

        // And a stream that behaves is not caught by either: a block of a
        // thousand interfaces is a real host, not an attack.
        let mut blocks = Blocks::new();
        let mut arrived = 0;
        let mut host = String::from("===afterm===\n1.0 1.0\n");
        for index in 0..1000 {
            host.push_str(&format!("  eth{index}: 500 0 0 0 0 0 0 0 700 0 0 0 0 0 0 0\n"));
        }
        host.push_str("===afterm===\n");
        blocks.feed(host.as_bytes(), |_| arrived += 1).unwrap();
        assert_eq!(arrived, 1);
    }

    /// A flood of nothing.
    ///
    /// The byte limit alone did not catch this: empty lines were retained and
    /// charged nothing, so the parser accepted 100,000 of them, accounted for
    /// zero bytes, and held 100,000 `String`s. Both limits now bite.
    #[test]
    fn refuses_a_flood_of_empty_lines_that_costs_nothing_to_send() {
        let mut blocks = Blocks::new();
        let flood = "\n".repeat(1024);
        let mut fed = 0;
        let outcome = loop {
            if let Err(reason) = blocks.feed(flood.as_bytes(), |_| unreachable!()) {
                break reason;
            }
            fed += 1024;
            assert!(fed <= MAX_ROWS + 1024, "an endless stream of newlines was accepted");
        };
        assert!(outcome.contains("block"), "{outcome}");
    }

    /// A line that never ends is caught by its own limit whether or not it
    /// ever becomes a row.
    #[test]
    fn a_row_count_limit_does_not_replace_the_byte_limits() {
        // Rows well under the count, well over the bytes.
        let mut blocks = Blocks::new();
        let line = format!("{}\n", "y".repeat(MAX_LINE - 1));
        let outcome = loop {
            if let Err(reason) = blocks.feed(line.as_bytes(), |_| unreachable!()) {
                break reason;
            }
        };
        assert!(outcome.contains("block"), "{outcome}");
        assert!(blocks.block.len() < MAX_ROWS, "the byte limit fired first, as it should");
    }

    /// Diagnostics are bounded, and the stream is drained to the end anyway:
    /// stopping the read is what leaves a child blocked on a full pipe.
    #[test]
    fn a_capped_read_keeps_a_prefix_and_still_empties_the_pipe() {
        struct Counted<'a> {
            source: std::io::Cursor<&'a [u8]>,
            reads: usize,
        }
        impl Read for Counted<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.reads += 1;
                self.source.read(buffer)
            }
        }

        let noise = vec![b'x'; 200 * 1024];
        let mut source = Counted { source: std::io::Cursor::new(&noise), reads: 0 };
        let (text, truncated) = read_capped(&mut source, 1024);
        assert_eq!(text.len(), 1024);
        assert!(truncated);
        // Read to the end rather than abandoned partway: 200 KiB in 8 KiB
        // chunks plus the final empty read.
        assert_eq!(source.reads, 200 * 1024 / (8 * 1024) + 1);

        let mut small = std::io::Cursor::new(&b"all of it"[..]);
        let (text, truncated) = read_capped(&mut small, 1024);
        assert_eq!(text, "all of it");
        assert!(!truncated);
    }

    #[test]
    fn survives_a_host_that_has_no_proc_filesystem() {
        let block = lines("bash: /proc/uptime: No such file or directory");
        let (mut cpu, mut net) = (None, None);
        let metrics = parse_block(&block, &mut cpu, &mut net);
        assert_eq!(metrics.cpu, None);
        assert_eq!(metrics.total_memory, None);
    }
}
