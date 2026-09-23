// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fuchsia_async::TimeoutExt;
use fuchsia_async::net::TcpStream;

use futures::{AsyncReadExt, AsyncWriteExt, TryFutureExt};
use std::net;

const FETCH_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(10);

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("failed to create socket")]
    CreateSocket(#[source] std::io::Error),
    #[error("failed to bind socket to device {interface_name}")]
    BindSocket {
        interface_name: String,
        #[source]
        err: std::io::Error,
    },
    #[error("failed to open TCP stream")]
    ConnectTcpStream(#[source] std::io::Error),
    #[error("timed out connecting TCP stream")]
    ConnectTcpStreamTimeout,
    #[error("failed to write to TCP stream")]
    WriteTcpStream(#[source] std::io::Error),
    #[error("timed out writing data to TCP stream")]
    WriteTcpStreamTimeout,
    #[error("failed to read response from TCP stream")]
    ReadTcpStream(#[source] std::io::Error),
    #[error("timed out reading response from TCP stream")]
    ReadTcpStreamTimeout,
    #[error("failed to parse string from UTF-8 bytes")]
    ParseUtf8(#[from] std::string::FromUtf8Error),
    #[error("response header malformed: {first_line}")]
    MalformedHeader { first_line: String },
    #[error("failed to parse status code")]
    ParseStatusCode(#[source] std::num::ParseIntError),
}

impl FetchError {
    /// Returns a short, simplified string describing the error.
    /// Each string should only take at most 14 characters, else the row labels for the
    /// `fetch_results` time series in internal visualization tool would be truncated.
    pub fn short_name(&self) -> String {
        let (name, os_err) = match self {
            Self::CreateSocket(err) => ("CreateSock", err.raw_os_error()),
            Self::BindSocket { err, .. } => ("BindSock", err.raw_os_error()),
            Self::ConnectTcpStream(err) => ("ConnectTcp", err.raw_os_error()),
            Self::ConnectTcpStreamTimeout => return "ConnTcpTimeout".to_string(),
            Self::WriteTcpStream(err) => ("WriteTcp", err.raw_os_error()),
            Self::WriteTcpStreamTimeout => return "WriteTcpTOut".to_string(),
            Self::ReadTcpStream(err) => ("ReadTcp", err.raw_os_error()),
            Self::ReadTcpStreamTimeout => return "ReadTcpTimeout".to_string(),
            Self::ParseUtf8(_) => return "ParseUtf8".to_string(),
            Self::MalformedHeader { .. } => return "BadHeader".to_string(),
            Self::ParseStatusCode(_) => return "ParseStatus".to_string(),
        };

        if let Some(code) = os_err { format!("{name}_{code}") } else { name.to_string() }
    }
}

pub(crate) fn fetch_result_short_name(result: &Result<u16, FetchError>) -> String {
    match result {
        Ok(code) => format!("Completed_{}", code),
        Err(e) => format!("e_{}", e.short_name()),
    }
}

fn http_request(path: &str, host: &str) -> String {
    [
        &format!("HEAD {path} HTTP/1.1"),
        &format!("host: {host}"),
        "connection: close",
        "user-agent: fuchsia reachability probe",
    ]
    .join("\r\n")
        + "\r\n\r\n"
}

async fn fetch<FA: FetchAddr + std::marker::Sync>(
    interface_name: &str,
    host: &str,
    path: &str,
    addr: &FA,
) -> Result<u16, FetchError> {
    let timeout = zx::MonotonicInstant::after(FETCH_TIMEOUT);
    let addr = addr.as_socket_addr();
    let socket = socket2::Socket::new(
        match addr {
            net::SocketAddr::V4(_) => socket2::Domain::IPV4,
            net::SocketAddr::V6(_) => socket2::Domain::IPV6,
        },
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .map_err(FetchError::CreateSocket)?;
    socket.bind_device(Some(interface_name.as_bytes())).map_err(|err| FetchError::BindSocket {
        interface_name: interface_name.to_string(),
        err,
    })?;
    let mut stream = TcpStream::connect_from_raw(socket, addr)
        .map_err(FetchError::ConnectTcpStream)?
        .map_err(FetchError::ConnectTcpStream)
        .on_timeout(timeout, || Err(FetchError::ConnectTcpStreamTimeout))
        .await?;
    let message = http_request(path, host);
    stream
        .write_all(message.as_bytes())
        .map_err(FetchError::WriteTcpStream)
        .on_timeout(timeout, || Err(FetchError::WriteTcpStreamTimeout))
        .await?;

    let mut bytes = Vec::new();
    let _: usize = stream
        .read_to_end(&mut bytes)
        .map_err(FetchError::ReadTcpStream)
        .on_timeout(timeout, || Err(FetchError::ReadTcpStreamTimeout))
        .await?;
    let resp = String::from_utf8(bytes)?;
    let first_line = resp.split("\r\n").next().expect("split always returns at least one item");
    if let [http, code, ..] = first_line.split(' ').collect::<Vec<_>>().as_slice() {
        if !http.starts_with("HTTP/") {
            return Err(FetchError::MalformedHeader { first_line: first_line.to_string() });
        }
        Ok(code.parse().map_err(FetchError::ParseStatusCode)?)
    } else {
        Err(FetchError::MalformedHeader { first_line: first_line.to_string() })
    }
}

pub trait FetchAddr {
    fn as_socket_addr(&self) -> net::SocketAddr;
}

impl FetchAddr for net::SocketAddr {
    fn as_socket_addr(&self) -> net::SocketAddr {
        *self
    }
}

impl FetchAddr for net::IpAddr {
    fn as_socket_addr(&self) -> net::SocketAddr {
        net::SocketAddr::from((*self, 80))
    }
}

#[async_trait]
pub trait Fetch {
    async fn fetch<FA: FetchAddr + std::marker::Sync>(
        &self,
        interface_name: &str,
        host: &str,
        path: &str,
        addr: &FA,
    ) -> Result<u16, FetchError>;
}

pub struct Fetcher;

#[async_trait]
impl Fetch for Fetcher {
    async fn fetch<FA: FetchAddr + std::marker::Sync>(
        &self,
        interface_name: &str,
        host: &str,
        path: &str,
        addr: &FA,
    ) -> Result<u16, FetchError> {
        fetch(interface_name, host, path, addr).await
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use anyhow::Context;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::pin::pin;

    use fuchsia_async::net::TcpListener;
    use futures::future::Fuse;
    use futures::io::BufReader;
    use futures::{AsyncBufReadExt, FutureExt, StreamExt};
    use test_case::test_case;

    fn server(
        code: u16,
    ) -> anyhow::Result<(SocketAddr, Fuse<impl futures::Future<Output = Vec<String>>>)> {
        let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
        let listener = TcpListener::bind(&addr).context("binding TCP")?;
        let addr = listener.local_addr()?;

        let server_fut = async move {
            let timeout = zx::MonotonicInstant::after(FETCH_TIMEOUT);
            let mut incoming = listener.accept_stream();
            if let Some(result) = incoming
                .next()
                .on_timeout(timeout, || panic!("timeout waiting for connection"))
                .await
            {
                let (stream, _addr) = result.expect("accept incoming TCP connection");
                let mut stream = BufReader::new(stream);
                let mut request = Vec::new();
                loop {
                    let mut s = String::new();
                    let _: usize = stream
                        .read_line(&mut s)
                        .on_timeout(timeout, || panic!("timeout waiting to read data"))
                        .await
                        .expect("read data");
                    if s == "\r\n" {
                        break;
                    }
                    request.push(s.trim().to_string());
                }
                let data = format!("HTTP/1.1 {} OK\r\n\r\n", code);
                stream
                    .write_all(data.as_bytes())
                    .on_timeout(timeout, || panic!("timeout waiting to write response"))
                    .await
                    .expect("reply to request");
                request
            } else {
                Vec::new()
            }
        }
        .fuse();

        Ok((addr, server_fut))
    }

    #[test_case("http://reachability.test/", 200; "base path 200")]
    #[test_case("http://reachability.test/path/", 200; "sub path 200")]
    #[test_case("http://reachability.test/", 400; "base path 400")]
    #[test_case("http://reachability.test/path/", 400; "sub path 400")]
    #[fuchsia::test]
    async fn test_fetch(url_str: &'static str, code: u16) -> anyhow::Result<()> {
        let url = url::Url::parse(url_str)?;
        let (addr, server_fut) = server(code)?;
        let domain = url.host().expect("no host").to_string();
        let path = url.path().to_string();

        let mut fetch_fut = pin!(fetch("", &domain, &path, &addr).fuse());

        let mut server_fut = pin!(server_fut);

        let mut request = None;
        let result = loop {
            futures::select! {
                req = server_fut => request = Some(req),
                result = fetch_fut => break result
            };
        };

        assert!(result.is_ok(), "Expected OK, got: {result:?}");
        assert_eq!(result.ok(), Some(code));
        let request = request.expect("no request body");
        assert!(request.contains(&format!("HEAD {path} HTTP/1.1")));
        assert!(request.contains(&format!("host: {domain}")));

        Ok(())
    }
}
