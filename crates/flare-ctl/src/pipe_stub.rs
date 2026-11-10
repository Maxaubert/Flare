//! The control channel endpoint off Windows: not built yet. macOS: phase 4 of the port plan (a Unix
//! socket with peer-uid checks). Until then the server refuses to start and every connect fails,
//! so the app runs without the channel and clients report that Flare is unreachable.

use std::convert::Infallible;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::client::{Client, ClientError};
use crate::framing::Wire;
use crate::session::{Dispatcher, ServerConfig};

/// Environment variable naming the channel (the same name as on Windows).
pub const ENV_PIPE: &str = "FLARE_CTL_PIPE";

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the Flare control channel is not built on this platform yet",
    )
}

/// The channel name for `id` and `scope`. macOS: phase 4 of the port plan picks the socket path.
pub fn pipe_name(id: &str, scope: Option<&str>) -> String {
    match scope {
        None => format!("flare-ctl-{id}"),
        Some(s) => format!("flare-ctl-{id}-{s}"),
    }
}

/// Who this process runs as. macOS: phase 4 of the port plan (the uid); None until then.
pub fn user_sid() -> Option<String> {
    None
}

/// The channel of the current user, None until phase 4.
pub fn user_pipe_name(scope: Option<&str>) -> Option<String> {
    user_sid().map(|id| pipe_name(&id, scope))
}

/// The channel a client should use: `FLARE_CTL_PIPE` when set and non-empty, else the user's default.
pub fn default_pipe_name() -> Option<String> {
    match std::env::var(ENV_PIPE) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => user_pipe_name(None),
    }
}

/// One end of a connected channel. None can exist until phase 4.
pub struct PipeEnd(Infallible);

impl Wire for PipeEnd {
    fn poll_read(&self, _buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.0 {}
    }

    fn write_all(&self, _bytes: &[u8]) -> io::Result<()> {
        match self.0 {}
    }
}

/// The running server. None can exist until phase 4: `start` always fails.
pub struct Server(Infallible);

impl Server {
    pub fn start(
        _name: &str,
        _config: ServerConfig,
        _dispatcher: Arc<dyn Dispatcher>,
    ) -> io::Result<Server> {
        Err(unsupported())
    }

    pub fn publish(&self, _event: &str, _params: Value, _tab: Option<&str>) -> usize {
        match self.0 {}
    }

    pub fn publish_to_owner(&self, _event: &str, _params: Value, _owner: &str) -> usize {
        match self.0 {}
    }

    pub fn client_count(&self) -> usize {
        match self.0 {}
    }

    pub fn pipe(&self) -> &str {
        match self.0 {}
    }

    pub fn stop(self) -> usize {
        match self.0 {}
    }
}

pub fn connect(_name: &str, _timeout: Duration) -> Result<Client<PipeEnd>, ClientError> {
    Err(ClientError::Io(unsupported()))
}

pub fn connect_to_server(
    _name: &str,
    _timeout: Duration,
    _pid: u32,
) -> Result<Client<PipeEnd>, ClientError> {
    Err(ClientError::Io(unsupported()))
}

pub fn connect_default(_timeout: Duration) -> Result<Client<PipeEnd>, ClientError> {
    Err(ClientError::Io(unsupported()))
}
