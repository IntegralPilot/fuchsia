// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_async as fasync;
use futures::prelude::*;
use std::net::SocketAddr;

/// Describes the possible errors to return for an HTTP request.
#[derive(Debug, PartialEq, Eq)]
enum HttpError {
    NotFound,
    BadRequest,
}

/// Verifies that an HTTP request contains a "GET /", and that's it.
fn verify_raw_http_request(buf: &[u8], msg_size: usize) -> Result<(), HttpError> {
    if let Some(first_line) =
        std::str::from_utf8(&buf[..msg_size]).ok().and_then(|s| s.lines().next())
    {
        let mut parts = first_line.split_whitespace();
        if parts.next() == Some("GET") && parts.next() == Some("/") {
            Ok(())
        } else {
            Err(HttpError::NotFound)
        }
    } else {
        Err(HttpError::BadRequest)
    }
}

/// Runs a barebones HTTP server that serves the authorized_keys on port 9797.
/// This is not using any third party code so as to avoid increasing binary size on devices. As
/// such, the verification of the incoming request is also very barebones. We read maximum 1024kb of
/// a request (so we expect something small), and only expect a `GET /` as a request.
pub async fn run_http_server() -> Result<(), Error> {
    log::info!("Setting up http server for authorized_keys");
    let addr = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 9797));
    let listener = fasync::net::TcpListener::bind(&addr)?;
    let scope = fasync::Scope::new();
    let mut stream = listener.accept_stream();
    log::info!("Trying to load file from /data/ssh/authorized_keys");
    let response =
        match fuchsia_fs::file::read_in_namespace_to_string("/data/ssh/authorized_keys").await {
            Ok(c) => format!("HTTP/1.0 200 OK\r\n\r\n{c}"),
            Err(e) => {
                log::warn!("Unable to load file from /data/ssh/authorized_keys: {e}");
                format!("HTTP/1.0 500 Internal Server Error\r\n\r\nUnable to read file: {e}")
            }
        };
    while let Some(r) = stream.try_next().await? {
        let (mut sock, _addr) = r;
        let response_clone = response.clone();
        scope.spawn(async move {
            let mut buf = [0u8; 1024];
            let response = match sock.read(&mut buf).await {
                Ok(n) => match verify_raw_http_request(&buf, n) {
                    Ok(()) => response_clone,
                    Err(HttpError::NotFound) => "HTTP/1.0 404 Not Found\r\n\r\n".to_owned(),
                    Err(HttpError::BadRequest) => "HTTP/1.0 400 Bad Request\r\n\r\n".to_owned(),
                },
                Err(e) => {
                    log::error!("error reading from socket: {e:?}");
                    return;
                }
            };
            if let Err(e) = sock.write_all(response.as_bytes()).await {
                log::error!("error writing to socket: {e:?}");
            }
        });
    }
    scope.join().await;
    log::info!("Closing server");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_raw_http_request_valid() {
        let req = "GET / HTTP/1.1\r\n\r\n";
        assert!(verify_raw_http_request(req.as_bytes(), req.len()).is_ok());
    }

    #[test]
    fn test_verify_raw_http_request_not_found() {
        let req = "GET /foo HTTP/1.1\r\n\r\n";
        assert_eq!(verify_raw_http_request(req.as_bytes(), req.len()), Err(HttpError::NotFound));
    }

    #[test]
    fn test_verify_raw_http_request_bad_request() {
        let req = &[0, 1, 2, 3];
        assert_eq!(verify_raw_http_request(req, req.len()), Err(HttpError::NotFound));
    }
}
