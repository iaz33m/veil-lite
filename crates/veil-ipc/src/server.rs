//! Server run by the engine: one thread accepting, one thread per connection.

use crate::framing::{read_frame, write_frame};
use crate::protocol::{Request, Response, PROTOCOL_VERSION};
use crate::transport::{self, Stream};
use crate::ENGINE_VERSION;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use parking_lot::Mutex;
use rand::RngCore;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use veil_core::paths::{write_private, Paths};
use veil_core::EngineEvent;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 32;

/// Answers requests. Implemented by the engine.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, req: Request) -> Response;
    fn subscribe(&self) -> Receiver<EngineEvent>;
}

pub struct Server {
    inner: Arc<Inner>,
    accept: Mutex<Option<JoinHandle<()>>>,
}

struct Inner {
    paths: Paths,
    token: String,
    handler: Arc<dyn Handler>,
    stop: AtomicBool,
    connections: AtomicUsize,
}

impl Server {
    /// Write a fresh token, bind the socket and start accepting connections.
    pub fn start(paths: &Paths, handler: Arc<dyn Handler>) -> anyhow::Result<Server> {
        paths.ensure()?;
        let listener = transport::bind(paths)?;
        let mut raw = [0u8; 32];
        rand::rng().fill_bytes(&mut raw);
        let token = hex::encode(raw);
        write_private(&paths.token_file(), token.as_bytes())?;

        let inner = Arc::new(Inner {
            paths: paths.clone(),
            token,
            handler,
            stop: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
        });
        let accept_inner = Arc::clone(&inner);
        let accept = std::thread::Builder::new().name("veil-ipc-accept".into()).spawn(move || {
            for conn in listener.incoming() {
                if accept_inner.stop.load(Ordering::SeqCst) {
                    break;
                }
                let stream = match conn {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("ipc accept failed: {e}");
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                };
                if accept_inner.connections.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    tracing::warn!("ipc: too many connections, refusing one");
                    continue;
                }
                accept_inner.connections.fetch_add(1, Ordering::SeqCst);
                let conn_inner = Arc::clone(&accept_inner);
                let spawned = std::thread::Builder::new().name("veil-ipc-conn".into()).spawn(move || {
                    if let Err(e) = conn_inner.serve(stream) {
                        if !matches!(e.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe) {
                            tracing::debug!("ipc connection ended: {e}");
                        }
                    }
                    conn_inner.connections.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    accept_inner.connections.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
        tracing::info!("ipc listening on {}", inner.paths.socket_name());
        Ok(Server { inner, accept: Mutex::new(Some(accept)) })
    }

    /// Stop accepting. Open connections end when their clients disconnect or the process exits.
    pub fn stop(&self) {
        if self.inner.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        // Wake the accept loop.
        let _ = transport::connect(&self.inner.paths);
        if let Some(handle) = self.accept.lock().take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(self.inner.paths.token_file());
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Inner {
    fn serve(&self, mut stream: Stream) -> io::Result<()> {
        if !transport::same_user(&stream) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "peer is another user"));
        }
        stream.set_send_timeout(Some(SEND_TIMEOUT))?;
        stream.set_recv_timeout(Some(HELLO_TIMEOUT))?;
        match read_frame::<_, Request>(&mut stream)? {
            Request::Hello { token, protocol } if constant_time_eq(token.as_bytes(), self.token.as_bytes()) => {
                if protocol != PROTOCOL_VERSION {
                    let message = format!("protocol {protocol} not supported (engine speaks {PROTOCOL_VERSION})");
                    write_frame(&mut stream, &Response::Error { message })?;
                    return Ok(());
                }
                write_frame(
                    &mut stream,
                    &Response::Hello { engine_version: ENGINE_VERSION.into(), protocol: PROTOCOL_VERSION },
                )?;
            }
            _ => {
                write_frame(&mut stream, &Response::Error { message: "authentication failed".into() })?;
                return Ok(());
            }
        }
        // Authenticated: clients may keep the connection open indefinitely.
        stream.set_recv_timeout(None)?;
        loop {
            let req: Request = read_frame(&mut stream)?;
            match req {
                Request::Subscribe => {
                    let events = self.handler.subscribe();
                    write_frame(&mut stream, &Response::Ok)?;
                    return self.stream_events(stream, events);
                }
                Request::Hello { .. } => {
                    write_frame(&mut stream, &Response::Error { message: "already authenticated".into() })?
                }
                req => {
                    let shutdown = req == Request::Shutdown;
                    let response = self.handler.handle(req);
                    write_frame(&mut stream, &response)?;
                    if shutdown {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn stream_events(&self, mut stream: Stream, events: Receiver<EngineEvent>) -> io::Result<()> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match events.recv_timeout(Duration::from_millis(500)) {
                Ok(ev) => write_frame(&mut stream, &Response::Event(ev))?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use crate::framing::{read_frame, write_frame};
    use veil_core::engine::EventBus;

    struct Echo {
        bus: EventBus,
    }

    impl Handler for Echo {
        fn handle(&self, req: Request) -> Response {
            match req {
                Request::Status => Response::Ok,
                Request::GetPolicy => Response::Policy { yaml: "version: 1\n".into() },
                Request::SetPolicy { yaml } => Response::Policy { yaml },
                Request::Pause { minutes } => {
                    self.bus.publish(EngineEvent::Paused { remaining_secs: minutes as u64 * 60 });
                    Response::Ok
                }
                _ => Response::Error { message: "unsupported".into() },
            }
        }

        fn subscribe(&self) -> Receiver<EngineEvent> {
            self.bus.subscribe()
        }
    }

    fn start() -> (tempfile::TempDir, Paths, Server, Arc<Echo>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path());
        let handler = Arc::new(Echo { bus: EventBus::default() });
        let server = Server::start(&paths, handler.clone()).unwrap();
        (dir, paths, server, handler)
    }

    #[test]
    fn requests_round_trip() {
        let (_dir, paths, server, _) = start();
        let mut c = Client::connect(&paths).unwrap();
        assert_eq!(c.engine_version(), ENGINE_VERSION);
        assert_eq!(c.call(&Request::Status).unwrap(), Response::Ok);
        assert_eq!(c.call(&Request::GetPolicy).unwrap(), Response::Policy { yaml: "version: 1\n".into() });
        let big = "k".repeat(100_000);
        assert_eq!(c.call(&Request::SetPolicy { yaml: big.clone() }).unwrap(), Response::Policy { yaml: big });
        server.stop();
    }

    #[cfg(unix)]
    #[test]
    fn socket_and_token_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, paths, _server, _) = start();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&paths.token_file()), 0o600);
        assert_eq!(mode(std::path::Path::new(&paths.socket_name())), 0o600);
        assert_eq!(std::fs::read_to_string(paths.token_file()).unwrap().len(), 64);
    }

    #[test]
    fn wrong_token_is_rejected() {
        let (_dir, paths, _server, _) = start();
        let mut s = transport::connect(&paths).unwrap();
        write_frame(&mut s, &Request::Hello { token: "0".repeat(64), protocol: PROTOCOL_VERSION }).unwrap();
        let r: Response = read_frame(&mut s).unwrap();
        assert_eq!(r, Response::Error { message: "authentication failed".into() });
        // The server closed the connection.
        assert!(write_frame(&mut s, &Request::Status).is_err() || read_frame::<_, Response>(&mut s).is_err());

        std::fs::write(paths.token_file(), "nope").unwrap();
        assert!(Client::connect(&paths).is_err());
    }

    #[test]
    fn requests_before_hello_are_rejected() {
        let (_dir, paths, _server, _) = start();
        let mut s = transport::connect(&paths).unwrap();
        write_frame(&mut s, &Request::Status).unwrap();
        let r: Response = read_frame(&mut s).unwrap();
        assert!(matches!(r, Response::Error { .. }));
    }

    #[test]
    fn oversize_frame_closes_the_connection() {
        let (_dir, paths, _server, _) = start();
        let mut c = Client::connect(&paths).unwrap();
        use std::io::Write;
        c_stream_write(&mut c, &((crate::MAX_FRAME + 10) as u32).to_le_bytes());
        // Connection is dropped by the server: the next call fails.
        std::thread::sleep(Duration::from_millis(100));
        assert!(c.call(&Request::Status).is_err());
        // The server keeps serving others.
        let mut c2 = Client::connect(&paths).unwrap();
        assert_eq!(c2.call(&Request::Status).unwrap(), Response::Ok);

        fn c_stream_write(c: &mut Client, bytes: &[u8]) {
            c.raw_stream().write_all(bytes).unwrap();
        }
    }

    #[test]
    fn subscription_streams_events() {
        let (_dir, paths, _server, _) = start();
        let mut sub = Client::subscribe(&paths).unwrap();
        let mut c = Client::connect(&paths).unwrap();
        c.call(&Request::Pause { minutes: 15 }).unwrap();
        assert_eq!(sub.next(), Some(EngineEvent::Paused { remaining_secs: 900 }));
    }

    #[test]
    fn second_server_on_the_same_socket_is_refused_and_stale_sockets_are_reclaimed() {
        let (_dir, paths, server, handler) = start();
        assert!(Server::start(&paths, handler.clone()).is_err());
        server.stop();
        drop(server);
        // A leftover socket file (crash) must not prevent a restart.
        #[cfg(unix)]
        std::fs::write(paths.socket_name(), b"").unwrap();
        let server = Server::start(&paths, handler).unwrap();
        let mut c = Client::connect(&paths).unwrap();
        assert_eq!(c.call(&Request::Status).unwrap(), Response::Ok);
        server.stop();
    }
}
