//! Client used by the desktop app (and `veil-engine status`).

use crate::framing::{read_frame, write_frame};
use crate::protocol::{Request, Response, PROTOCOL_VERSION};
use crate::transport::{self, Stream};
use anyhow::{bail, Context};
use interprocess::local_socket::traits::Stream as _;
use std::time::Duration;
use veil_core::paths::Paths;
use veil_core::EngineEvent;

/// Upper bound for one request/response round trip (permission prompts can take a while).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Client {
    stream: Stream,
    engine_version: String,
}

impl Client {
    /// Connect to the engine of this user and authenticate with the token file.
    pub fn connect(paths: &Paths) -> anyhow::Result<Client> {
        let token = std::fs::read_to_string(paths.token_file())
            .with_context(|| format!("reading the engine token at {}", paths.token_file().display()))?;
        let stream = transport::connect(paths).context("connecting to the Veil engine")?;
        stream.set_recv_timeout(Some(CALL_TIMEOUT))?;
        stream.set_send_timeout(Some(Duration::from_secs(10)))?;
        let mut client = Client { stream, engine_version: String::new() };
        match client.call(&Request::Hello { token: token.trim().to_string(), protocol: PROTOCOL_VERSION })? {
            Response::Hello { engine_version, protocol } => {
                if protocol != PROTOCOL_VERSION {
                    bail!("engine speaks protocol {protocol}, this app speaks {PROTOCOL_VERSION}");
                }
                client.engine_version = engine_version;
                Ok(client)
            }
            Response::Error { message } => bail!("engine refused the connection: {message}"),
            other => bail!("unexpected handshake response: {other:?}"),
        }
    }

    pub fn engine_version(&self) -> &str {
        &self.engine_version
    }

    /// Send one request and wait for its response.
    pub fn call(&mut self, req: &Request) -> anyhow::Result<Response> {
        write_frame(&mut self.stream, req).context("sending to the engine")?;
        read_frame(&mut self.stream).context("reading the engine's response")
    }

    /// Open a dedicated connection that streams engine events.
    pub fn subscribe(paths: &Paths) -> anyhow::Result<Subscription> {
        let mut client = Client::connect(paths)?;
        match client.call(&Request::Subscribe)? {
            Response::Ok => {}
            Response::Error { message } => bail!("subscription refused: {message}"),
            other => bail!("unexpected subscription response: {other:?}"),
        }
        client.stream.set_recv_timeout(None)?;
        Ok(Subscription { stream: client.stream })
    }
}

/// Engine events, in order, until the engine goes away (then the iterator ends).
pub struct Subscription {
    stream: Stream,
}

impl Iterator for Subscription {
    type Item = EngineEvent;

    fn next(&mut self) -> Option<EngineEvent> {
        loop {
            match read_frame::<_, Response>(&mut self.stream) {
                Ok(Response::Event(ev)) => return Some(ev),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

#[cfg(test)]
impl Client {
    pub(crate) fn raw_stream(&mut self) -> &mut Stream {
        &mut self.stream
    }
}
