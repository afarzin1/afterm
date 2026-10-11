//! A directory that removes itself, so a failed test leaves nothing behind.
//!
//! Test-only, and shared: the transports both write real files while they are
//! being exercised, and neither should be able to leave any on the machine
//! running the suite.

pub struct Dir(std::path::PathBuf);

impl Dir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "afterm-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
