// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test tools for serving TUF repositories.

use crate::repo::Repository;
use anyhow::{Context as _, Error, bail, format_err};
use chrono::Utc;
use fidl_fuchsia_pkg_ext::{
    MirrorConfig, MirrorConfigBuilder, RepositoryConfig, RepositoryStorageType,
};
use fuchsia_async::net::TcpListener;
use fuchsia_async::{self as fasync, Task};
use fuchsia_repo::body::Body;
use fuchsia_sync::Mutex;
use fuchsia_url::RepositoryUrl;
use futures::future::BoxFuture;
use futures::prelude::*;
use http::Uri;
use http_body_util::BodyExt;
use http_sse::{Event, EventSender, SseResponseCreator};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, header};
use std::convert::{Infallible, TryInto as _};
use std::io::{Cursor, Read as _, Seek as _};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub mod responder;

trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}
impl<T> AsyncReadWrite for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}

/// Domains with known keys and certificates for testing.
pub enum Domain {
    /// `test.fuchsia.com` and `localhost`.
    TestFuchsiaCom,
    /// `*.fuchsia-updates.googleusercontent.com`.
    WildcardFuchsiaUpdatesGoogleusercontentCom,
}

/// A builder to construct a test repository server.
pub struct ServedRepositoryBuilder {
    repo: Arc<Repository>,
    response_overriders: Vec<Arc<dyn HttpResponder>>,
    bind_addr: IpAddr,
    bind_port: u16,
    https_domain: Option<Domain>,
}

/// Override how a `ServedRepository` responds to requests.
/// Useful for injecting failures.
pub trait HttpResponder: 'static + Send + Sync {
    /// `response` is what the server would have responded with.
    fn respond<'a>(
        &'a self,
        request: &'a Request<Body>,
        response: Response<Body>,
    ) -> BoxFuture<'a, Response<Body>>;
}

/// An entry in the server's log of requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The request's method.
    pub method: http::Method,
    /// The request's path.
    pub path: String,
    /// The response's status code.
    pub status: http::StatusCode,
}

impl ServedRepositoryBuilder {
    pub(crate) fn new(repo: Arc<Repository>) -> Self {
        ServedRepositoryBuilder {
            repo,
            response_overriders: vec![],
            bind_addr: Ipv6Addr::UNSPECIFIED.into(),
            bind_port: 0,
            https_domain: None,
        }
    }

    /// Override how the `ServedRepositoryBuilder` responds to requests.
    ///
    /// Requests are passed through responders in the order in which the responders are added to
    /// this builder.
    pub fn response_overrider(mut self, responder: impl HttpResponder) -> Self {
        self.response_overriders.push(Arc::new(responder));
        self
    }

    /// Serve the repository over https via a domain known by test certificates
    /// and keys.
    pub fn use_https_domain(mut self, domain: Domain) -> Self {
        self.https_domain = Some(domain);
        self
    }

    /// Bind the tcp listener to the provided ip address. Binds to Ipv6Addr::UNSPECIFIED by
    /// default.
    pub fn bind_to_addr(mut self, addr: impl Into<IpAddr>) -> Self {
        self.bind_addr = addr.into();
        self
    }

    /// Bind the tcp listener to the provided port. Binds to 0 (allowing system to select a port) by
    /// default.
    pub fn bind_to_port(mut self, port: u16) -> Self {
        self.bind_port = port;
        self
    }

    /// Spawn the server on the current executor, returning a handle to manage the server.
    pub fn start(self) -> Result<ServedRepository, Error> {
        let (listener, addr) = {
            let addr = SocketAddr::new(self.bind_addr, self.bind_port);
            let listener = TcpListener::bind(&addr).context("bind")?;
            let local_addr = listener.local_addr().context("local_addr")?;
            (listener, local_addr)
        };

        let listener = listener
            .accept_stream()
            .map_err(Error::from)
            .map_ok(|(conn, _addr)| fuchsia_hyper::TcpStream { stream: conn });

        let connection_attempts = Arc::new(AtomicU64::new(0));
        #[allow(clippy::type_complexity)]
        let connections: Pin<
            Box<dyn Stream<Item = Result<Pin<Box<dyn AsyncReadWrite>>, Error>> + Send>,
        > = if let Some(ref https_domain) = self.https_domain {
            // build a server configuration using a test CA and cert chain
            let (certs, key) = match https_domain {
                Domain::TestFuchsiaCom => (
                    parse_cert_chain(&include_bytes!("../certs/test.fuchsia.com.certchain")[..]),
                    parse_private_key(&include_bytes!("../certs/test.fuchsia.com.rsa")[..]),
                ),
                Domain::WildcardFuchsiaUpdatesGoogleusercontentCom => (
                    parse_cert_chain(
                        &include_bytes!(
                            "../certs/wildcard.fuchsia-updates.googleusercontent.com.certchain"
                        )[..],
                    ),
                    parse_private_key(
                        &include_bytes!(
                            "../certs/wildcard.fuchsia-updates.googleusercontent.com.rsa"
                        )[..],
                    ),
                ),
            };
            let mut tls_config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap();

            // Configure ALPN and prefer H2 over HTTP/1.1.
            tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

            let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
            let connection_attempts = Arc::clone(&connection_attempts);

            // wrap incoming tcp streams
            listener
                .and_then(move |conn| {
                    connection_attempts.fetch_add(1, Ordering::SeqCst);
                    tls_acceptor.accept(conn).map(|res| match res {
                        Ok(conn) => Ok(Pin::new(Box::new(conn)) as Pin<Box<dyn AsyncReadWrite>>),
                        Err(e) => Err(Error::from(e)),
                    })
                })
                .boxed()
        } else {
            let connection_attempts = Arc::clone(&connection_attempts);
            listener
                .map_ok(move |conn| {
                    connection_attempts.fetch_add(1, Ordering::SeqCst);
                    Pin::new(Box::new(conn)) as Pin<Box<dyn AsyncReadWrite>>
                })
                .boxed()
        };

        let root = self.repo.path();
        let response_overriders = Arc::new(self.response_overriders);

        let (auto_response_creator, auto_event_sender) =
            SseResponseCreator::with_additional_buffer_size(10);
        let auto_response_creator = Arc::new(auto_response_creator);

        let history = Arc::new(Mutex::new(vec![]));
        let history_clone = Arc::clone(&history);

        let (stop, rx_stop) = futures::channel::oneshot::channel();
        let rx_stop = rx_stop.map(|res| res.unwrap_or(())).shared();

        let server = Task::spawn(async move {
            let mut connections = connections.fuse();
            let mut tasks = futures::stream::FuturesUnordered::new();
            loop {
                futures::select! {
                    conn = connections.next() => {
                        match conn {
                            Some(Ok(conn)) => {
                                let root = root.clone();
                                let response_overriders = Arc::clone(&response_overriders);
                                let auto_response_creator = Arc::clone(&auto_response_creator);
                                let history = Arc::clone(&history_clone);
                                let rx_stop = rx_stop.clone();
                                tasks.push(fuchsia_async::Task::spawn(async move {
                                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                                        let (parts, _body) = req.into_parts();
                                        let req = Request::from_parts(parts, Body::empty());
                                        let version = req.version();
                                        let method = req.method().to_owned();
                                        let path = req.uri().path().to_owned();
                                        let headers = req.headers().clone();
                                        let history = Arc::clone(&history);
                                        ServedRepository::handle_tuf_repo_request_infallible(
                                            root.clone(),
                                            Arc::clone(&response_overriders),
                                            Arc::clone(&auto_response_creator),
                                            req,
                                        )
                                        .inspect(move |x| {
                                            history.lock().push(HistoryEntry {
                                                    method: method.clone(),
                                                    path: path.clone(),
                                                    status: x.status(),
                                                });
                                            println!(
                                                "{} [http repo] {:?} {} {} {:?} => {}",
                                                Utc::now().format("%T.%6f"),
                                                version,
                                                method,
                                                path,
                                                headers,
                                                x.status()
                                            )
                                        })
                                        .map(Ok::<_, Infallible>)
                                    });
                                    let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);
                                    let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(conn), service);
                                    let mut conn = std::pin::pin!(conn);
                                    futures::select! {
                                        res = conn.as_mut().fuse() => {
                                            if let Err(e) = res {
                                                log::warn!("error serving repo connection: {e}");
                                            }
                                        }
                                        _ = rx_stop.clone() => {
                                            conn.as_mut().graceful_shutdown();
                                            let _ = conn.await;
                                        }
                                    }
                                }));
                            }
                            Some(Err(e)) => {
                                log::error!("error accepting repo connection: {e}");
                            }
                            None => break,
                        }
                    }
                    _ = rx_stop.clone() => break,
                    _ = tasks.next() => {}
                }
            }
            while tasks.next().await.is_some() {}
        });

        Ok(ServedRepository {
            repo: self.repo,
            stop,
            server,
            addr,
            https_domain: self.https_domain,
            auto_event_sender,
            connection_attempts,
            history,
        })
    }
}

fn parse_cert_chain(mut bytes: &[u8]) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    rustls_pemfile::certs(&mut bytes).collect::<Result<Vec<_>, _>>().expect("certs to parse")
}

fn parse_private_key(mut bytes: &[u8]) -> rustls::pki_types::PrivateKeyDer<'static> {
    rustls_pemfile::private_key(&mut bytes)
        .expect("private keys to parse")
        .expect("one private key")
}

/// A [`Repository`] being served over HTTP.
pub struct ServedRepository {
    repo: Arc<Repository>,
    stop: futures::channel::oneshot::Sender<()>,
    server: Task<()>,
    addr: SocketAddr,
    auto_event_sender: EventSender,
    connection_attempts: Arc<AtomicU64>,
    https_domain: Option<Domain>,
    history: Arc<Mutex<Vec<HistoryEntry>>>,
}

impl ServedRepository {
    fn scheme(&self) -> &'static str {
        if self.https_domain.is_some() { "https" } else { "http" }
    }
    /// Request the given path served by the repository over HTTP.
    pub async fn get(&self, path: impl AsRef<str>) -> Result<Vec<u8>, Error> {
        let url = format!("{}/{}", self.local_url(), path.as_ref());
        get(url).await
    }

    /// Returns the URL that can be used to connect to this repository from this device.
    pub fn local_url(&self) -> String {
        format!("{}://localhost:{}", self.scheme(), self.addr.port())
    }

    /// Returns a sorted vector of all packages contained in this repository.
    pub async fn list_packages(&self) -> Result<Vec<crate::repo::PackageEntry>, Error> {
        let targets_json = self.get("targets.json").await?;
        let mut packages = crate::repo::iter_packages(Cursor::new(targets_json))?
            .collect::<Result<Vec<_>, _>>()?;
        packages.sort_unstable();
        Ok(packages)
    }

    /// Generates a [`MirrorConfigBuilder`] that points to this served repository.
    pub fn get_mirror_config_builder(&self) -> MirrorConfigBuilder {
        MirrorConfigBuilder::new(self.local_url().parse::<Uri>().unwrap()).unwrap()
    }

    /// Generates a [`MirrorConfig`] that points to this served repository.
    fn get_mirror_config(&self, subscribe: bool) -> MirrorConfig {
        self.get_mirror_config_builder().subscribe(subscribe).build()
    }

    /// Generate a [`RepositoryConfig`] suitable for configuring a package resolver to use this
    /// served repository.
    pub fn make_repo_config(&self, url: RepositoryUrl) -> RepositoryConfig {
        self.repo.make_repo_config(url, Some(self.get_mirror_config(false)), false)
    }

    /// Generate a [`RepositoryConfig`] suitable for configuring a package resolver to use this
    /// served repository. Set subscribe on the mirror configs to true.
    pub fn make_repo_config_with_subscribe(&self, url: RepositoryUrl) -> RepositoryConfig {
        self.repo.make_repo_config(url, Some(self.get_mirror_config(true)), false)
    }

    /// Generate a [`RepositoryConfig`] suitable for configuring a package resolver to use this
    /// served repository with local mirroring enabled.
    // TODO(https://fxbug.dev/42137938) delete this method once pkg-resolver can fetch metadata from a LocalMirror.
    pub fn make_repo_config_with_local_mirror(&self, url: RepositoryUrl) -> RepositoryConfig {
        self.repo.make_repo_config(url, Some(self.get_mirror_config(false)), true)
    }

    /// Generate a [`RepositoryConfig`] that permits persisting metadata.
    pub fn make_repo_config_with_persistent_storage(&self, url: RepositoryUrl) -> RepositoryConfig {
        self.repo
            .make_repo_config_builder(url)
            .add_mirror(self.get_mirror_config(false))
            .use_local_mirror(false)
            .repo_storage_type(RepositoryStorageType::Persistent)
            .build()
    }

    /// Send an SSE event to all clients subscribed to /auto.
    pub async fn send_auto_event(&self, event: &Event) {
        self.auto_event_sender.send(event).await
    }

    /// Waits until `send_auto_event` would attempt to send an `Event` to exactly
    /// `n` clients. Panics if extra clients are connected.
    pub async fn wait_for_n_connected_auto_clients(&self, n: usize) {
        loop {
            let connected = self.auto_event_sender.client_count().await;
            match connected.cmp(&n) {
                std::cmp::Ordering::Equal => {
                    break;
                }
                std::cmp::Ordering::Greater => {
                    panic!("ServedRepository too many auto clients connected.");
                }
                _ => {}
            }
            fasync::Timer::new(Duration::from_millis(10)).await;
        }
    }

    /// Errors all currently existing /auto `Response<Body>` streams.
    pub async fn drop_all_auto_clients(&self) {
        self.auto_event_sender.drop_all_clients().await
    }

    /// Gracefully signal the server to stop and returns a future that resolves when it terminates.
    pub fn stop(self) -> impl Future<Output = ()> {
        self.stop.send(()).expect("remote end to still be open");
        self.server
    }

    /// Number of connection attempts.
    pub fn connection_attempts(&self) -> u64 {
        self.connection_attempts.load(Ordering::SeqCst)
    }

    /// The log of requests.
    pub fn history(&self) -> Arc<Mutex<Vec<HistoryEntry>>> {
        self.history.clone()
    }

    async fn handle_tuf_repo_request_infallible(
        repo: PathBuf,
        response_overriders: Arc<Vec<Arc<dyn HttpResponder>>>,
        auto_response_creator: Arc<SseResponseCreator>,
        req: Request<Body>,
    ) -> Response<Body> {
        let mut response = Self::handle_tuf_repo_request(repo, auto_response_creator, &req)
            .await
            .unwrap_or_else(|e| {
                eprintln!("hyper tuf server error creating response for request {req:?}: {e:#}");
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from("Error creating response".to_owned().into_bytes()))
                    .unwrap()
            });

        for responder in response_overriders.iter() {
            response = responder.respond(&req, response).await
        }

        response
    }

    async fn handle_tuf_repo_request(
        repo: PathBuf,
        auto_response_creator: Arc<SseResponseCreator>,
        req: &Request<Body>,
    ) -> Result<Response<Body>, Error> {
        let fail =
            |status: StatusCode| Response::builder().status(status).body(Body::empty()).unwrap();
        if *req.method() != Method::GET {
            return Ok(fail(StatusCode::NOT_FOUND));
        } else if req.uri().query().is_some() {
            return Ok(fail(StatusCode::BAD_REQUEST));
        }

        let uri_path = Path::new(req.uri().path());

        // don't let queries escape the repo root.
        if uri_path.components().any(|component| component == std::path::Component::ParentDir) {
            return Ok(fail(StatusCode::NOT_FOUND));
        }

        let response = if uri_path.to_str() == Some("/auto") {
            let resp = auto_response_creator.create().await;
            let (parts, body) = resp.into_parts();
            let body = body.filter_map(|res| async move {
                match res {
                    Ok(frame) => frame.into_data().ok().map(Ok),
                    Err(e) => Some(Err(e)),
                }
            });
            Response::from_parts(parts, Body::wrap_stream(body))
        } else {
            let fs_path = repo.join(uri_path.strip_prefix("/").unwrap_or(uri_path));
            // TODO(https://fxbug.dev/42150717) synchronous IO in an async context.
            let mut file = match std::fs::File::open(fs_path) {
                Ok(file) => file,
                Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::from("File did not exist".to_owned().into_bytes()))
                        .unwrap());
                }
                Err(e) => Err(e).context("opening file")?,
            };

            if let Some(range) = req.headers().get(http::header::RANGE) {
                make_range_response(file, range).context("error making range response")?
            } else {
                let mut body = vec![];
                file.read_to_end(&mut body).context("reading repo file")?;
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, body.len())
                    .body(Body::from(body))
                    .unwrap()
            }
        };

        Ok(response)
    }
}

// TODO(https://fxbug.dev/42150593) use specific HTTP status codes for errors instead of mapping everything to
// INTERNAL_SERVER_ERROR.
fn make_range_response(
    mut file: std::fs::File,
    range: &http::HeaderValue,
) -> Result<Response<Body>, Error> {
    let HttpRange { first_byte_pos, last_byte_pos } =
        range.try_into().context("parse range header")?;
    let file_size = file.metadata().context("file metadata")?.len();
    // TODO(https://fxbug.dev/42150593) return 416 if the range is invalid
    file.seek(std::io::SeekFrom::Start(first_byte_pos)).context("seeking file")?;
    let mut data = vec![0; 1 + last_byte_pos as usize - first_byte_pos as usize];
    file.read_exact(&mut data).context("reading file for range request")?;

    Ok(Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_LENGTH, data.len())
        .header(
            header::CONTENT_RANGE,
            format!("bytes {first_byte_pos}-{last_byte_pos}/{file_size}"),
        )
        .body(Body::from(data))
        .unwrap())
}

/// Parsed value of an HTTP request "Range" headers
pub struct HttpRange {
    first_byte_pos: u64,
    last_byte_pos: u64,
}

impl HttpRange {
    /// The first byte pos of the Range header.
    pub fn first_byte_pos(&self) -> u64 {
        self.first_byte_pos
    }

    /// The last byte pos of the Range header.
    pub fn last_byte_pos(&self) -> u64 {
        self.last_byte_pos
    }
}

// TODO(https://fxbug.dev/42150593) use a spec compliant parser
impl TryFrom<&http::HeaderValue> for HttpRange {
    type Error = anyhow::Error;

    fn try_from(range: &http::HeaderValue) -> Result<Self, Self::Error> {
        let range = range.to_str().context("range header should be ascii")?;
        let range = if let Some(range) = range.strip_prefix("bytes=") {
            range
        } else {
            bail!("range header should start with 'bytes='");
        };
        let dash =
            range.find('-').ok_or_else(|| anyhow::anyhow!("range header should have dash"))?;
        let (first, last) = range.split_at(dash);
        if last.len() < 2 {
            bail!("range header last_byte_pos empty");
        }
        let first = first.parse().context("valid range first_byte_pos")?;
        let last = last[1..].parse().context("valid range last_byte_pos")?;

        if first > last {
            bail!("first_byte_pos {} > last_byte_pos {}", first, last);
        }

        Ok(HttpRange { first_byte_pos: first, last_byte_pos: last })
    }
}

async fn get(url: impl AsRef<str>) -> Result<Vec<u8>, Error> {
    let request = Request::get(url.as_ref())
        .body(http_body_util::Full::<hyper::body::Bytes>::new("".into()))
        .map_err(Error::from)?;
    let client = fuchsia_hyper::new_client();
    let response = client.request(request).await?;

    if response.status() != StatusCode::OK {
        return Err(format_err!("unexpected status code: {:?}", response.status()));
    }

    let body = response.into_body().collect().await?.to_bytes();

    Ok(body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageBuilder;
    use crate::repo::RepositoryBuilder;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    #[ignore]
    async fn test_serve_empty_hangs_on_last_get() {
        let repo = Arc::new(RepositoryBuilder::new().build().await.unwrap());
        let served_repo = repo.server().start().unwrap();

        // contains no packages.
        let packages = served_repo.list_packages().await.unwrap();
        assert_eq!(packages, vec![]);

        // no '..' allowed.
        assert_matches!(served_repo.get("blobs/../root.json").await, Err(_));

        // getting a known file fetches something.
        let bytes = served_repo.get("targets.json").await.unwrap();
        assert_ne!(bytes, Vec::<u8>::new());

        // even if it doesn't go through the helper function.
        let url = format!("{}/targets.json", served_repo.local_url());
        let also_bytes = get(&url).await.unwrap();
        assert_eq!(bytes, also_bytes);

        // requests fail after stopping the server
        served_repo.stop().await;

        // FIXME(49247): this often flakes by hanging
        assert_matches!(get(url).await, Err(_));
    }

    #[fuchsia::test]
    async fn test_serve_empty() {
        let repo = Arc::new(RepositoryBuilder::new().build().await.unwrap());
        let served_repo = repo.server().start().unwrap();

        // contains no packages.
        let packages = served_repo.list_packages().await.unwrap();
        assert_eq!(packages, vec![]);

        // no '..' allowed.
        assert_matches!(served_repo.get("blobs/../root.json").await, Err(_));

        // getting a known file fetches something.
        let bytes = served_repo.get("targets.json").await.unwrap();
        assert_ne!(bytes, Vec::<u8>::new());

        // even if it doesn't go through the helper function.
        let url = format!("{}/targets.json", served_repo.local_url());
        let also_bytes = get(&url).await.unwrap();
        assert_eq!(bytes, also_bytes);
    }

    #[fuchsia::test]
    async fn test_serve_packages() {
        let same_contents = "same contents";
        let repo = RepositoryBuilder::new()
            .add_package(
                PackageBuilder::new("rolldice")
                    .add_resource_at("bin/rolldice", "#!/boot/bin/sh\necho 4\n".as_bytes())
                    .add_resource_at(
                        "meta/rolldice.cml",
                        r#"{"program":{"binary":"bin/rolldice"}}"#.as_bytes(),
                    )
                    .add_resource_at("data/duplicate_a", "same contents".as_bytes())
                    .build()
                    .await
                    .unwrap(),
            )
            .add_package(
                PackageBuilder::new("fortune")
                    .add_resource_at(
                        "bin/fortune",
                        "#!/boot/bin/sh\necho ask again later\n".as_bytes(),
                    )
                    .add_resource_at(
                        "meta/fortune.cml",
                        r#"{"program":{"binary":"bin/fortune"}}"#.as_bytes(),
                    )
                    .add_resource_at("data/duplicate_b", same_contents.as_bytes())
                    .add_resource_at("data/duplicate_c", same_contents.as_bytes())
                    .build()
                    .await
                    .unwrap(),
            )
            .build()
            .await
            .unwrap();
        let repo = Arc::new(repo);

        let local_packages = repo.list_packages().unwrap();

        let served_repository = repo.server().start().unwrap();
        let served_packages = served_repository.list_packages().await.unwrap();
        assert_eq!(local_packages, served_packages);
    }
}
