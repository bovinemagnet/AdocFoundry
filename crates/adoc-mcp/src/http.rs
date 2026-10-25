//! MCP's Streamable HTTP transport, for clients that connect to a running server rather than
//! spawning one. It listens on a loopback address only, and answers only requests that carry
//! the server's token, so neither a web page nor another local user can call its tools.

use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use bytes::Bytes;
use http::{header, HeaderMap, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt as _, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::server::Server;

/// The path the transport is served at.
pub const PATH: &str = "/mcp";

/// The environment variable a client's token is taken from, when set and not empty.
pub const TOKEN_VARIABLE: &str = "ADOC_MCP_TOKEN";

/// `address` as a socket address, when it names a loopback host: `127.0.0.1:8765`,
/// `[::1]:8765` or `localhost:8765`. Port 0 picks any free port.
pub fn loopback(address: &str) -> Result<SocketAddr, String> {
    let parsed = match address.rsplit_once(':') {
        Some(("localhost", port)) => port
            .parse()
            .map(|port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .map_err(|_| format!("`{address}` has no valid port")),
        _ => address
            .parse::<SocketAddr>()
            .map_err(|_| format!("`{address}` is not an address and port, such as 127.0.0.1:8765")),
    }?;
    if !parsed.ip().is_loopback() {
        return Err(format!(
            "`{address}` is not a loopback address; the HTTP transport serves this machine only"
        ));
    }
    Ok(parsed)
}

/// The token clients must present: `from_environment` when it is set and not empty, otherwise
/// 32 random bytes in hex.
pub fn token(from_environment: Option<String>) -> Result<String, String> {
    if let Some(token) = from_environment.filter(|token| !token.is_empty()) {
        return Ok(token);
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| format!("no random token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Whether `headers` carry `Authorization: Bearer <token>`. The comparison takes the same time
/// wherever the given token first differs, so timing does not reveal how much of it matched.
fn authorised(headers: &HeaderMap, token: &str) -> bool {
    let Some(given) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (given, token) = (given.as_bytes(), token.as_bytes());
    given.len() == token.len()
        && given
            .iter()
            .zip(token)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn status(code: StatusCode, message: &'static str) -> Response<BoxBody<Bytes, Infallible>> {
    let mut response = Response::new(Full::new(Bytes::from_static(message.as_bytes())).boxed());
    *response.status_mut() = code;
    if code == StatusCode::UNAUTHORIZED {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
    }
    response
}

/// Serves `server` on `address` until Ctrl-C or SIGTERM, every session sharing its engine.
pub async fn serve(server: Server, address: SocketAddr, token: String) -> Result<(), String> {
    let listener = TcpListener::bind(address)
        .await
        .map_err(|error| format!("cannot listen on {address}: {error}"))?;
    let bound = listener.local_addr().map_err(|error| error.to_string())?;
    eprintln!("adoc-mcp: listening on http://{bound}{PATH}");

    let cancel = CancellationToken::new();
    // rmcp's default `Host` check accepts loopback hosts only, against DNS rebinding.
    let config =
        StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token());
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let token: Arc<str> = token.into();

    let shutdown = shutdown();
    tokio::pin!(shutdown);
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => continue,
            },
            () = &mut shutdown => break,
        };
        let service = service.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |request: Request<Incoming>| {
                let service = service.clone();
                let token = token.clone();
                async move {
                    // Refused before rmcp sees the request, so nothing about the session leaks.
                    if request.uri().path() != PATH {
                        return Ok::<_, Infallible>(status(StatusCode::NOT_FOUND, "not found"));
                    }
                    if !authorised(request.headers(), &token) {
                        return Ok(status(StatusCode::UNAUTHORIZED, "unauthorised"));
                    }
                    Ok(service.handle(request).await)
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .await;
        });
    }
    cancel.cancel();
    Ok(())
}

/// Completes on Ctrl-C, or on SIGTERM where there is one.
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_addresses_are_served() {
        assert_eq!(
            loopback("127.0.0.1:8765").unwrap(),
            "127.0.0.1:8765".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            loopback("[::1]:0").unwrap(),
            "[::1]:0".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            loopback("localhost:9000").unwrap(),
            "127.0.0.1:9000".parse::<SocketAddr>().unwrap()
        );
        assert!(loopback("0.0.0.0:8765").is_err());
        assert!(loopback("192.168.1.2:8765").is_err());
        assert!(loopback("example.com:80").is_err());
        assert!(loopback("8765").is_err());
    }

    #[test]
    fn the_token_comes_from_the_environment_or_is_random() {
        assert_eq!(token(Some("secret".into())).unwrap(), "secret");
        let generated = token(Some(String::new())).unwrap();
        assert_eq!(generated.len(), 64);
        assert!(generated.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(generated, token(None).unwrap());
    }

    #[test]
    fn only_the_bearer_token_is_authorised() {
        let headers = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            headers
        };
        assert!(authorised(&headers("Bearer abc"), "abc"));
        assert!(!authorised(&headers("Bearer abd"), "abc"));
        assert!(!authorised(&headers("Bearer ab"), "abc"));
        assert!(!authorised(&headers("Basic abc"), "abc"));
        assert!(!authorised(&HeaderMap::new(), "abc"));
    }
}
