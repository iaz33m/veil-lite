//! The connection to `veil-engine` over local IPC (blocking; call from `spawn_blocking` or a thread).

use parking_lot::Mutex;
use veil_core::paths::Paths;
use veil_ipc::{Client, Request, Response};

pub struct EngineLink {
    paths: Paths,
    /// One cached connection for short requests, reopened after any error.
    client: Mutex<Option<Client>>,
}

impl EngineLink {
    pub fn new(paths: Paths) -> Self {
        EngineLink { paths, client: Mutex::new(None) }
    }

    /// Send a request on the shared connection, reconnecting once if it went stale.
    pub fn call(&self, req: Request) -> Result<Response, String> {
        let mut guard = self.client.lock();
        let mut last_error = String::new();
        for _ in 0..2 {
            if guard.is_none() {
                match Client::connect(&self.paths) {
                    Ok(c) => *guard = Some(c),
                    Err(e) => return Err(unreachable(&e)),
                }
            }
            let client = guard.as_mut().expect("client just set");
            match client.call(&req) {
                Ok(Response::Error { message }) => return Err(message),
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    *guard = None;
                    last_error = unreachable(&e);
                }
            }
        }
        Err(last_error)
    }

    /// Send a request on its own connection, for calls that can block for a long time (a permission
    /// prompt) so they don't hold up status polling.
    pub fn call_alone(&self, req: Request) -> Result<Response, String> {
        let mut client = Client::connect(&self.paths).map_err(|e| unreachable(&e))?;
        match client.call(&req) {
            Ok(Response::Error { message }) => Err(message),
            Ok(resp) => Ok(resp),
            Err(e) => Err(unreachable(&e)),
        }
    }

    /// True when an engine answers.
    pub fn is_up(&self) -> bool {
        matches!(self.call(Request::Status), Ok(Response::Status(_)))
    }

    /// Drop the cached connection (after the engine restarts).
    pub fn reset(&self) {
        *self.client.lock() = None;
    }
}

fn unreachable(e: &anyhow::Error) -> String {
    format!("Veil’s engine isn’t answering ({e:#}).")
}

/// `Ok` for an acknowledgement, an error for anything else.
pub fn expect_ok(resp: Response) -> Result<(), String> {
    match resp {
        Response::Ok => Ok(()),
        other => Err(unexpected(&other)),
    }
}

pub fn unexpected(resp: &Response) -> String {
    format!("The engine sent an unexpected reply: {resp:?}")
}
