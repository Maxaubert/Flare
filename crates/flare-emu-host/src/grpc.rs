//! The gRPC client for one emulator's `EmulatorController`, on localhost with its token (rule 1362).
//!
//! The token goes only into the `authorization` header, marked sensitive (tonic and hyper then never print it); it is
//! never logged, never in a hello fact, an error or an answer.

use std::time::Duration;

use flare_device::emu::discovery::Token;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

use crate::pb::emulator_controller_client::EmulatorControllerClient;

/// Adds the bearer token to every call.
#[derive(Clone)]
pub struct Auth(MetadataValue<Ascii>);

impl Interceptor for Auth {
    fn call(&mut self, mut r: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        r.metadata_mut().insert("authorization", self.0.clone());
        Ok(r)
    }
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Auth(hidden)")
    }
}

pub type Client = EmulatorControllerClient<InterceptedService<Channel, Auth>>;

/// The largest message read: a PNG snapshot or, in the fallback, a frame in the message (1080 x 2400 RGBA is 10 MB).
pub const MAX_MESSAGE: usize = 64 << 20;

/// The header value for a token, marked sensitive. Err when the token is not header text.
pub fn auth(token: &Token) -> Result<Auth, String> {
    let mut v: MetadataValue<Ascii> = token
        .bearer()
        .parse()
        .map_err(|_| "the emulator's token is not header text".to_string())?;
    v.set_sensitive(true);
    Ok(Auth(v))
}

/// Connects to the emulator's gRPC port on this machine. The error never holds the token.
pub async fn connect(port: u16, token: &Token) -> Result<Client, String> {
    let auth = auth(token)?;
    let ch = Endpoint::from_shared(format!("http://localhost:{port}"))
        .map_err(|e| format!("gRPC port {port}: {e}"))?
        .connect_timeout(Duration::from_secs(5))
        .tcp_nodelay(true)
        .connect()
        .await
        .map_err(|e| format!("gRPC port {port} did not answer: {e}"))?;
    Ok(EmulatorControllerClient::with_interceptor(ch, auth).max_decoding_message_size(MAX_MESSAGE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1362: the header carries the token, marked sensitive, and nothing prints it.
    #[test]
    fn the_token_goes_only_into_a_sensitive_header() {
        let t = Token::new("c2VjcmV0").unwrap();
        let a = auth(&t).unwrap();
        assert!(a.0.is_sensitive());
        assert_eq!(a.0.to_str().unwrap(), "Bearer c2VjcmV0");
        let shown = format!("{a:?} {:?} {t:?} {t}", a.0);
        assert!(!shown.contains("c2VjcmV0"), "{shown}");
        let mut i = a.clone();
        let r = i.call(tonic::Request::new(())).unwrap();
        assert_eq!(
            r.metadata().get("authorization").map(|v| v.is_sensitive()),
            Some(true)
        );
        assert!(auth(&Token::new("bad\nvalue").unwrap()).is_err());
    }
}
