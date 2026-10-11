use serde::Serialize;

/// Every field is optional and every `None` renders as an em dash in the status
/// bar. A metric afterm cannot measure is shown as unknown, never as a number.
///
/// These are all read off the host at the other end of an SSH connection —
/// `/proc` and `df` through `remote.rs` — plus the latency of the connection
/// carrying them. There was a `LocalMetrics` here that sampled this machine
/// through `sysinfo`, and it did not come back with the local shells: nothing
/// in the status bar is about the machine you are already sitting at, so a
/// local shell shows an empty bar rather than readings of itself.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub cpu: Option<f32>,
    pub used_memory: Option<u64>,
    pub total_memory: Option<u64>,
    pub disk_used: Option<u64>,
    pub disk_total: Option<u64>,
    pub download_rate: Option<f64>,
    pub upload_rate: Option<f64>,
    pub uptime: Option<u64>,
    pub latency: Option<f64>,
    /// Where the session's shell is, for the file pane to follow. Not a metric
    /// and not rendered in the status bar: it rides here because it is read
    /// out of the same `/proc` block by the same parser, and carrying it
    /// separately would mean a second poll of the same stream.
    pub directory: Option<String>,
}
