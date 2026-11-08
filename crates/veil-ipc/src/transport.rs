//! Socket names and low-level connect/bind.

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ConnectOptions, ListenerOptions, Name};
use std::io;
use std::time::Duration;
use veil_core::paths::Paths;

pub type Stream = LocalSocketStream;
pub type Listener = LocalSocketListener;

pub fn name(paths: &Paths) -> io::Result<Name<'static>> {
    let raw = paths.socket_name();
    #[cfg(windows)]
    {
        use interprocess::local_socket::GenericNamespaced;
        Ok(raw.to_ns_name::<GenericNamespaced>()?.into_owned())
    }
    #[cfg(not(windows))]
    {
        use interprocess::local_socket::GenericFilePath;
        Ok(std::path::PathBuf::from(raw).to_fs_name::<GenericFilePath>()?.into_owned())
    }
}

pub fn connect(paths: &Paths) -> io::Result<Stream> {
    use interprocess::ConnectWaitMode;
    ConnectOptions::new().name(name(paths)?).wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(3))).connect_sync()
}

/// Bind the engine socket. On Unix a leftover socket file is removed only if nothing answers on it.
pub fn bind(paths: &Paths) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        let path = std::path::PathBuf::from(paths.socket_name());
        if path.exists() {
            if connect(paths).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, "another Veil engine is already running"));
            }
            std::fs::remove_file(&path)?;
        }
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        let listener = ListenerOptions::new().name(name(paths)?).mode(0o600).create_sync()?;
        // Belt and braces: some platforms ignore the creation mode.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        Ok(listener)
    }
    #[cfg(not(unix))]
    {
        ListenerOptions::new().name(name(paths)?).create_sync()
    }
}

/// Peer runs as the same user (Unix); always true elsewhere (named pipe ACL + token).
pub fn same_user(stream: &Stream) -> bool {
    #[cfg(unix)]
    {
        use interprocess::local_socket::traits::StreamCommon;
        match stream.peer_creds() {
            Ok(creds) => creds.euid().is_none_or(|uid| uid == unsafe { libc::geteuid() }),
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = stream;
        true
    }
}
