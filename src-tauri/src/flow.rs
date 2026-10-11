//! Backpressure between whatever is producing terminal bytes and the interface
//! that has to draw them.
//!
//! Without this, a producer that outruns the renderer — `cat` of a large file,
//! a build log, a program in a loop — has nowhere to queue but the webview's
//! own message queue, which grows without bound. The interface knows how far
//! behind it is, because it is told when each chunk has been parsed, so it is
//! the side that decides: past a high-water mark it closes the gate, and once
//! it has caught up it opens it again.
//!
//! Closing the gate stops the reader, not the writer. Keystrokes travel a
//! different path in both transports, so Ctrl-C still reaches the far end
//! while a flood is being held back — which is the one thing that must keep
//! working, because it is how the flood gets stopped.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar, Mutex,
};
use tokio::sync::Notify;

/// One terminal's gate.
///
/// Two ways to wait on the same flag, because the two transports read in two
/// different ways: a pseudo-terminal is drained by a thread that blocks, and a
/// channel is drained by a task that cannot.
pub struct FlowGate {
    paused: AtomicBool,
    /// Paired with `resumed`, for the blocking side.
    state: Mutex<()>,
    resumed: Condvar,
    /// The same signal for the async side.
    woken: Notify,
}

impl FlowGate {
    pub fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            state: Mutex::new(()),
            resumed: Condvar::new(),
            woken: Notify::new(),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        // The lock is held across the store so that a waiter cannot read the
        // flag and settle down to wait in the window between the store and the
        // wake-up, which would leave it asleep with the gate already open.
        let _held = self.state.lock().unwrap();
        self.paused.store(paused, Ordering::SeqCst);
        if !paused {
            self.resumed.notify_all();
            self.woken.notify_waiters();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Blocks the calling thread until the gate is open.
    pub fn wait(&self) {
        let mut held = self.state.lock().unwrap();
        while self.paused.load(Ordering::SeqCst) {
            held = self.resumed.wait(held).unwrap();
        }
    }

    /// Yields the calling task until the gate is open.
    pub async fn opened(&self) {
        loop {
            // Registered before the flag is read, and explicitly: `notify_waiters`
            // wakes only waiters that already exist, so a waiter that checked
            // first would miss the wake-up it was about to wait for.
            let waiting = self.woken.notified();
            tokio::pin!(waiting);
            waiting.as_mut().enable();
            if !self.paused.load(Ordering::SeqCst) {
                return;
            }
            waiting.await;
        }
    }
}

impl Default for FlowGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn opens_by_default() {
        let gate = FlowGate::new();
        assert!(!gate.is_paused());
        // Nothing to wait for, so this returns rather than blocking.
        gate.wait();
    }

    #[test]
    fn a_blocked_reader_runs_again_when_the_gate_opens() {
        let gate = Arc::new(FlowGate::new());
        gate.set_paused(true);

        let waiter = Arc::clone(&gate);
        let handle = std::thread::spawn(move || {
            waiter.wait();
            true
        });

        // Still held: the thread must not have got through.
        std::thread::sleep(Duration::from_millis(50));
        assert!(!handle.is_finished(), "the gate let a reader through while closed");

        gate.set_paused(false);
        assert!(handle.join().unwrap());
    }

    #[tokio::test]
    async fn a_blocked_task_runs_again_when_the_gate_opens() {
        let gate = Arc::new(FlowGate::new());
        gate.set_paused(true);

        let waiter = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            waiter.opened().await;
            true
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished(), "the gate let a reader through while closed");

        gate.set_paused(false);
        assert!(task.await.unwrap());
    }

    #[tokio::test]
    async fn an_open_gate_never_waits() {
        let gate = FlowGate::new();
        // Would hang rather than fail if `opened` waited on an open gate.
        tokio::time::timeout(Duration::from_secs(5), gate.opened())
            .await
            .expect("an open gate must not make a reader wait");
    }
}
