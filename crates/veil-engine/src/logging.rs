//! Logging to stderr and to a size-capped file (`engine.log`, rotated once to `engine.1.log`).
//! Logs never contain message text or file contents; the engine only logs decisions and errors.

use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// Install the global subscriber. `level` is an `EnvFilter` directive such as `info` or `veil_core=debug`;
/// `RUST_LOG` wins when set.
pub fn init(log_file: Option<&Path>, level: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(level))?;
    let stderr = tracing_subscriber::fmt::layer().with_writer(io::stderr).with_target(false);
    let file = match log_file {
        Some(path) => Some(
            tracing_subscriber::fmt::layer().with_ansi(false).with_target(true).with_writer(CappedFile::open(path)?),
        ),
        None => None,
    };
    tracing_subscriber::registry().with(filter).with(stderr).with(file).try_init()?;
    Ok(())
}

/// A log file that rotates to `<name>.1.log` once it grows past [`MAX_LOG_BYTES`].
#[derive(Clone)]
pub struct CappedFile {
    inner: Arc<Mutex<FileState>>,
}

struct FileState {
    path: PathBuf,
    file: File,
    written: u64,
    max: u64,
}

impl CappedFile {
    pub fn open(path: &Path) -> io::Result<CappedFile> {
        Self::with_limit(path, MAX_LOG_BYTES)
    }

    pub fn with_limit(path: &Path, max: u64) -> io::Result<CappedFile> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata()?.len();
        Ok(CappedFile { inner: Arc::new(Mutex::new(FileState { path: path.to_path_buf(), file, written, max })) })
    }
}

impl FileState {
    fn rotate(&mut self) -> io::Result<()> {
        let old = self.path.with_extension("1.log");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&self.path, &old)?;
        self.file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

pub struct CappedWriter {
    inner: Arc<Mutex<FileState>>,
}

impl Write for CappedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut s = self.inner.lock();
        if s.written + buf.len() as u64 > s.max && s.written > 0 {
            s.rotate()?;
        }
        let n = s.file.write(buf)?;
        s.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().file.flush()
    }
}

impl<'a> MakeWriter<'a> for CappedFile {
    type Writer = CappedWriter;

    fn make_writer(&'a self) -> CappedWriter {
        CappedWriter { inner: Arc::clone(&self.inner) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_once_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.log");
        let f = CappedFile::with_limit(&path, 100).unwrap();
        let mut w = f.make_writer();
        for _ in 0..30 {
            w.write_all(b"0123456789\n").unwrap();
        }
        let current = std::fs::metadata(&path).unwrap().len();
        assert!(current <= 100, "current log is {current} bytes");
        assert!(dir.path().join("engine.1.log").exists());
    }
}
