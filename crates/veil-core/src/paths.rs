//! Where Veil keeps its files.
//!
//! | | config + policy | data (activity, models) | engine socket |
//! |---|---|---|---|
//! | macOS | `~/Library/Application Support/Veil` | same | `…/Veil/engine.sock` |
//! | Windows | `%APPDATA%\Veil` | `%LOCALAPPDATA%\Veil` | `\\.\pipe\veil-engine-<user>` |
//! | Linux | `$XDG_CONFIG_HOME/veil` | `$XDG_DATA_HOME/veil` | `$XDG_RUNTIME_DIR/veil/engine.sock` |
//!
//! `VEIL_HOME` overrides everything (tests, portable installs).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl Paths {
    pub fn detect() -> anyhow::Result<Paths> {
        if let Some(home) = std::env::var_os("VEIL_HOME") {
            return Ok(Paths::rooted(Path::new(&home)));
        }
        let base = directories::BaseDirs::new().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        let paths = if cfg!(target_os = "macos") {
            let dir = base.data_dir().join("Veil");
            Paths { config_dir: dir.clone(), data_dir: dir.clone(), runtime_dir: dir }
        } else if cfg!(windows) {
            Paths {
                config_dir: base.config_dir().join("Veil"),
                data_dir: base.data_local_dir().join("Veil"),
                runtime_dir: base.data_local_dir().join("Veil"),
            }
        } else {
            let runtime = base.runtime_dir().map(|d| d.join("veil")).unwrap_or_else(|| base.data_local_dir().join("veil"));
            Paths { config_dir: base.config_dir().join("veil"), data_dir: base.data_local_dir().join("veil"), runtime_dir: runtime }
        };
        Ok(paths)
    }

    /// Everything under one directory.
    pub fn rooted(root: &Path) -> Paths {
        Paths { config_dir: root.to_path_buf(), data_dir: root.to_path_buf(), runtime_dir: root.to_path_buf() }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [&self.config_dir, &self.data_dir, &self.runtime_dir, &self.models_dir()] {
            std::fs::create_dir_all(dir)?;
        }
        restrict_dir(&self.runtime_dir);
        Ok(())
    }

    pub fn policy_file(&self) -> PathBuf {
        self.config_dir.join("policy.yaml")
    }

    pub fn activity_file(&self) -> PathBuf {
        self.data_dir.join("activity.jsonl")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    pub fn log_file(&self) -> PathBuf {
        self.data_dir.join("engine.log")
    }

    /// Random per-boot token the app must present on the IPC socket.
    pub fn token_file(&self) -> PathBuf {
        self.runtime_dir.join("engine.token")
    }

    /// Local socket name: a filesystem path on Unix, a pipe name on Windows.
    pub fn socket_name(&self) -> String {
        if cfg!(windows) {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
            let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            if std::env::var_os("VEIL_HOME").is_some() {
                // Tests: unique per root.
                let tag = crate::shared::short_hash(self.runtime_dir.to_string_lossy().as_bytes());
                format!("veil-engine-{user}-{tag}")
            } else {
                format!("veil-engine-{user}")
            }
        } else {
            self.runtime_dir.join("engine.sock").to_string_lossy().into_owned()
        }
    }
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) {}

/// Write a file readable only by the current user.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(contents)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_paths_live_under_root() {
        let p = Paths::rooted(Path::new("/tmp/veil-test"));
        assert_eq!(p.policy_file(), Path::new("/tmp/veil-test/policy.yaml"));
        assert_eq!(p.models_dir(), Path::new("/tmp/veil-test/models"));
        assert!(p.socket_name().contains("veil"));
    }
}
