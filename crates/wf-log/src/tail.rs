//! Rotation-aware log tailing.
//!
//! [`LogTailer`] follows `EE.log` by polling: each [`poll`](LogTailer::poll)
//! reads whatever has been appended since the previous call and returns the
//! newly-completed lines. It transparently handles the game restarting (which
//! truncates or replaces the log) by detecting an inode change or a file that
//! has shrunk below the last read position, then re-reading from the start.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Follows a growing (and possibly rotating) log file.
pub struct LogTailer {
    path: PathBuf,
    pos: u64,
    inode: Option<u64>,
    /// Buffer holding a trailing partial line (no newline yet).
    partial: String,
}

impl LogTailer {
    /// Create a tailer starting from the **beginning** of the file (useful for
    /// reprocessing history).
    pub fn from_start(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            pos: 0,
            inode: None,
            partial: String::new(),
        }
    }

    /// Create a tailer positioned at the current **end** of the file, so only
    /// lines written after construction are returned. Falls back to the start
    /// if the file cannot be stat-ed yet.
    pub fn from_end(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (pos, inode) = match std::fs::metadata(&path) {
            Ok(m) => (m.len(), Some(m.ino())),
            Err(_) => (0, None),
        };
        Self {
            path,
            pos,
            inode,
            partial: String::new(),
        }
    }

    /// Read all lines appended since the last poll.
    ///
    /// Returns an empty vec when there is nothing new. A missing file is not an
    /// error (the game may not be running yet) — it also yields an empty vec.
    pub fn poll(&mut self) -> io::Result<Vec<String>> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };

        // Detect rotation/truncation: new inode, or file shorter than our cursor.
        let rotated = self.inode.is_some_and(|old| old != meta.ino());
        if rotated || meta.len() < self.pos {
            tracing::info!("EE.log rotated/truncated, re-reading from start");
            self.pos = 0;
            self.partial.clear();
        }
        self.inode = Some(meta.ino());

        if meta.len() == self.pos {
            return Ok(Vec::new());
        }

        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.pos))?;
        let mut buf = String::new();
        // Lossy decode guards against the occasional non-UTF-8 byte in the log.
        let mut bytes = Vec::new();
        let read = file.take(meta.len() - self.pos).read_to_end(&mut bytes)?;
        self.pos += read as u64;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        self.partial.push_str(&buf);
        Ok(self.drain_complete_lines())
    }

    /// Split the internal buffer into complete lines, retaining any trailing
    /// partial line for the next poll.
    fn drain_complete_lines(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Some(idx) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=idx).collect();
            lines.push(line.trim_end_matches(['\n', '\r']).to_string());
        }
        lines
    }

    /// The file being followed.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("wf-log-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn follows_appends_and_holds_partial_lines() {
        let path = temp_path("append");
        std::fs::write(&path, "1.0 Sys [Info]: first\n").unwrap();
        let mut t = LogTailer::from_start(&path);

        assert_eq!(t.poll().unwrap(), vec!["1.0 Sys [Info]: first"]);
        assert!(t.poll().unwrap().is_empty());

        // Append a complete line plus a partial (no newline yet).
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            write!(f, "2.0 Sys [Info]: second\n3.0 Sys [Info]: par").unwrap();
        }
        assert_eq!(t.poll().unwrap(), vec!["2.0 Sys [Info]: second"]);

        // Complete the partial line.
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "tial").unwrap();
        }
        assert_eq!(t.poll().unwrap(), vec!["3.0 Sys [Info]: partial"]);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn re_reads_after_truncation() {
        let path = temp_path("trunc");
        std::fs::write(&path, "1.0 Sys [Info]: old-a\n1.1 Sys [Info]: old-b\n").unwrap();
        let mut t = LogTailer::from_start(&path);
        assert_eq!(t.poll().unwrap().len(), 2);

        // Simulate the game restarting: rewrite (shorter) file.
        std::fs::write(&path, "0.0 Sys [Info]: fresh\n").unwrap();
        assert_eq!(t.poll().unwrap(), vec!["0.0 Sys [Info]: fresh"]);

        std::fs::remove_file(&path).ok();
    }
}
