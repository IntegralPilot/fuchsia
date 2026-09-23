// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::manager::RepositoryManager;
use crate::range::{ContentRange, Range};
use crate::repo_client::RepoClient;
use crate::repository::{Error as RepoError, RepoProvider};
use anyhow::Result;
use async_lock::RwLock as AsyncRwLock;
use chrono::Utc;
use derivative::Derivative;
use fuchsia_async as fasync;
use fuchsia_url::RepositoryUrl;
use futures::TryStreamExt;
use futures::future::Shared;
use futures::prelude::*;
use http::Uri;
use http_sse::{Event, EventSender, SseResponseCreator};
use hyper::header::RANGE;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};

pub use crate::body::{Body, BodySender};
use log::{error, info, warn};
use netext::{TcpListenerStream, TokioAsyncReadExt, TokioAsyncWrapper};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock as SyncRwLock, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

// FIXME: This value was chosen basically at random.
const AUTO_BUFFER_SIZE: usize = 10;

// FIXME: This value was chosen basically at random.
const MAX_PARSE_RETRIES: usize = 5000;

// FIXME: This value was chosen basically at random.
const PARSE_RETRY_DELAY: Duration = Duration::from_micros(100);

const REPOSITORY_PREFIX: &str = "<!doctype html>
<head>
  <link rel=\"stylesheet\" defer href=\"https://code.getmdl.io/1.3.0/material.indigo-pink.min.css\">
  <link rel=\"icon\" href=\"data:image/png;base64,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\">
  <script defer src=\"https://code.getmdl.io/1.3.0/material.min.js\"></script>
  <title>Package Repository</title>
  <style>
  body {
    margin: 10px;
  }
  h1 {
    color: #666;
  }
  </style>
</head>
<body>
<header><h1></img>Package Repositories</h1></header>
<ul>";

const REPOSITORY_SUFFIX: &str = "</ul>
</body>
</html>";

const PACKAGE_PREFIX: &str = "<!doctype html>
<head>
  <link rel=\"stylesheet\" defer href=\"https://code.getmdl.io/1.3.0/material.indigo-pink.min.css\">
  <link rel=\"icon\" href=\"data:image/png;base64,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\">
  <script defer src=\"https://code.getmdl.io/1.3.0/material.min.js\"></script>
  <title>repo1 Package Repository</title>
  <style>
  body {
    margin: 10px;
  }
  h1 {
    color: #666;
  }
  #package-table .merkle {
    font-family: monospace;
  }
  </style>
</head>
<body>
  <header><h1>repo1 Package Repository</h1></header>
  <div>
    <div>Version: <span>1234</span></div>
    <div>Expires: <span>2023-01-01...</span></div>
  </div>
  <div>
    <table id=package-table class=mdl-data-table>
      <thead>
        <tr>
          <th>Package</th>
          <th>Merkle</th>
        </tr>
      </thead>";

const PACKAGE_SUFFIX: &str = "</table>
</div>
</body>
</html>";

type SseResponseCreatorMap = SyncRwLock<HashMap<String, Arc<SseResponseCreator>>>;

/// RepositoryManager represents the web server that serves [Repositories](Repository) to a target.
#[derive(Debug)]
pub struct RepositoryServer {
    local_addr: SocketAddr,
    stop: futures::channel::oneshot::Sender<()>,
}

impl RepositoryServer {
    /// Gracefully signal the server to stop, and returns a future that resolves when the server
    /// terminates.
    pub fn stop(self) {
        self.stop.send(()).expect("remote end to still be open");
    }

    /// The [RepositoryServer] is listening on this address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns the URL that can be used to connect to this server.
    pub fn local_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Create a [RepositoryServerBuilder].
    pub fn builder(
        addr: SocketAddr,
        repo_manager: Arc<RepositoryManager>,
    ) -> RepositoryServerBuilder {
        RepositoryServerBuilder::new(addr, repo_manager)
    }
}

/// [RepositoryServerBuilder] constructs [RepositoryServer] instances.
pub struct RepositoryServerBuilder {
    addr: SocketAddr,
    repo_manager: Arc<RepositoryManager>,
}

impl RepositoryServerBuilder {
    /// Create a new RepositoryServerBuilder.
    pub fn new(addr: SocketAddr, repo_manager: Arc<RepositoryManager>) -> Self {
        Self { addr, repo_manager }
    }

    /// Construct a web server future, and return a [RepositoryServer] to manage the server.
    /// [RepositoryServer], and return a handle to manaserver and the web server task.
    pub async fn start(
        self,
    ) -> std::io::Result<(
        impl Future<Output = ()>,
        futures::channel::mpsc::UnboundedSender<Result<ConnectionStream>>,
        RepositoryServer,
    )> {
        let listener = TcpListener::bind(&self.addr).await.map_err(|e| {
            std::io::Error::other(format!("Cannot start server on {}: {e}", self.addr))
        })?;
        let local_addr = listener.local_addr()?;

        let (tx_stop_server, rx_stop_server) = futures::channel::oneshot::channel();
        let (tx_conns, rx_conns) = futures::channel::mpsc::unbounded();

        let server_fut =
            run_server(listener, rx_conns, rx_stop_server, Arc::clone(&self.repo_manager));

        Ok((server_fut, tx_conns, RepositoryServer { local_addr, stop: tx_stop_server }))
    }
}

/// Executor to help run tasks in the background, but still allow these tasks to be cleaned up when
/// the server is shut down.
#[derive(Debug, Default, Clone)]
struct TaskExecutor<T: 'static> {
    inner: Arc<Mutex<TaskExecutorInner<T>>>,
}

#[derive(Debug, Default)]
struct TaskExecutorInner<T> {
    next_task_id: u64,
    tasks: HashMap<u64, fasync::Task<T>>,
}

impl<T> TaskExecutor<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(TaskExecutorInner {
                next_task_id: 0,
                tasks: HashMap::new(),
            })),
        }
    }

    /// Spawn a task in the executor.
    pub fn spawn<F: Future<Output = T> + 'static>(&self, fut: F) {
        let fut_inner = Arc::clone(&self.inner);

        let mut inner = self.inner.lock().unwrap();

        // We could technically have a collision when the task id overflows, but if we spawned a
        // task once a nanosecond, we still wouldn't overflow for 584 years. But lets add an
        // assertion just to be safe.
        let task_id = inner.next_task_id;
        inner.next_task_id = inner.next_task_id.wrapping_add(1);
        assert!(!inner.tasks.contains_key(&task_id));

        let fut = async move {
            let res = fut.await;
            // Clean up our entry when the task completes.
            fut_inner.lock().unwrap().tasks.remove(&task_id);
            res
        };

        inner.tasks.insert(task_id, fasync::Task::local(fut));
    }
}

impl<T, F: Future<Output = T> + 'static> hyper::rt::Executor<F> for TaskExecutor<T> {
    fn execute(&self, fut: F) {
        self.spawn(fut);
    }
}

/// Starts the server loop.
async fn run_server(
    listener: TcpListener,
    tunnel_conns: futures::channel::mpsc::UnboundedReceiver<Result<ConnectionStream>>,
    server_stopped: futures::channel::oneshot::Receiver<()>,
    server_repo_manager: Arc<RepositoryManager>,
) {
    // Turn the shutdown signal into a shared future, so we can signal to the server to and all the
    // auto/ SSE processes, to initiate a graceful shutdown.
    let mut server_stopped = server_stopped.shared();

    // Merge the listener connections with the tunnel connections.
    let mut incoming = futures::stream::select(
        TcpListenerStream(listener)
            .map_ok(|value| ConnectionStream::Tcp(TcpStreamWithPeerAddr::from(value)))
            .map_err(Into::into),
        tunnel_conns,
    )
    .fuse();

    // Spawn all connections and related tasks in this executor.
    let executor = TaskExecutor::new();

    // Contains all the SSE services.
    let server_sse_response_creators = Arc::new(SyncRwLock::new(HashMap::new()));

    loop {
        let conn = futures::select! {
            conn = incoming.next() => {
                match conn {
                    Some(Ok(conn)) => conn,
                    Some(Err(err)) => {
                        error!("failed to accept connection: {:?}", err);
                        continue;
                    }
                    None => {
                        unreachable!(
                            "incoming stream has ended, which should be impossible \
                            according to async_net::TcpListener logs"
                        );
                    }
                }
            },
            _ = server_stopped => {
                break;
            },
        };

        let service_rx_stop = server_stopped.clone();
        let service_repo_manager = Arc::clone(&server_repo_manager);
        let service_sse_response_creators = Arc::clone(&server_sse_response_creators);

        executor.spawn(handle_connection(
            executor.clone(),
            service_rx_stop,
            service_repo_manager,
            service_sse_response_creators,
            conn,
        ));
    }
}

async fn handle_connection(
    executor: TaskExecutor<()>,
    server_stopped: Shared<futures::channel::oneshot::Receiver<()>>,
    repo_manager: Arc<RepositoryManager>,
    sse_response_creators: Arc<SseResponseCreatorMap>,
    conn: ConnectionStream,
) {
    let service_server_stopped = server_stopped.clone();
    let peer_addr = match &conn {
        ConnectionStream::Tcp(tcp_stream) => {
            if let Ok(addr) = tcp_stream.peer_addr {
                addr.to_string()
            } else {
                String::from("unknown")
            }
        }
        ConnectionStream::TargetTcp(target_tcp) => {
            format!("{target_tcp:?}")
        }
    };
    let builder = hyper_util::server::conn::auto::Builder::new(executor.clone());
    let conn = builder.serve_connection(
        hyper_util::rt::TokioIo::new(conn),
        service_fn(|req| {
            // Each request made by a connection is serviced by the
            // service_fn created from this scope, which is why there is
            // another cloning of the repository manager.
            let method = req.method().to_string();
            let path = req.uri().path().to_string();

            let peer_addr_clone = peer_addr.clone();
            handle_request(
                executor.clone(),
                service_server_stopped.clone(),
                Arc::clone(&repo_manager),
                Arc::clone(&sse_response_creators),
                req,
            )
            .map(move |(resp, content_range)| {
                info!(
                    "{} [ffx] {} {} {} => {} {}",
                    Utc::now().format("%T.%6f"),
                    peer_addr_clone,
                    method,
                    path,
                    resp.status(),
                    match content_range {
                        Some(ContentRange::Full { complete_len }) => complete_len.to_string(),
                        Some(ContentRange::Inclusive {
                            first_byte_pos,
                            last_byte_pos,
                            complete_len,
                        }) => {
                            format!("{first_byte_pos}-{last_byte_pos}/{complete_len}")
                        }
                        None => "-".into(),
                    },
                );
                Ok::<_, Infallible>(resp)
            })
        }),
    );

    let conn = GracefulConnection { conn, stop: server_stopped, shutting_down: false };

    if let Err(err) = conn.await {
        error!("Error while serving HTTP connection: {}", err);
    }
}

/// [GracefulConnection] will signal to the connection to shut down if we receive a shutdown signal
/// on the `stop` channel.
#[pin_project::pin_project]
struct GracefulConnection<'a, S>
where
    S: hyper::service::Service<Request<hyper::body::Incoming>, Response = Response<Body>>,
    S::Error: std::error::Error + Send + Sync + 'static,
    S::Future: 'static,
{
    #[pin]
    stop: Shared<futures::channel::oneshot::Receiver<()>>,

    /// The hyper connection.
    #[pin]
    conn: hyper_util::server::conn::auto::Connection<
        'a,
        hyper_util::rt::TokioIo<ConnectionStream>,
        S,
        TaskExecutor<()>,
    >,

    shutting_down: bool,
}

impl<'a, S> Future for GracefulConnection<'a, S>
where
    S: hyper::service::Service<Request<hyper::body::Incoming>, Response = Response<Body>>,
    S::Error: std::error::Error + Send + Sync + 'static,
    S::Future: 'static,
{
    type Output = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        if !*this.shutting_down {
            match this.stop.poll(cx) {
                Poll::Pending => {}
                Poll::Ready(_) => {
                    *this.shutting_down = true;

                    // Tell the connection to begin to gracefully shut down the connection.
                    // According to the [docs], we need to continue polling the connection until
                    // completion. This allows hyper to flush any send queues, or emit any
                    // HTTP-level shutdown messages. That would help the client to distinguish
                    // the server going away on purpose, or some other unexpected error.
                    //
                    // [docs]: https://docs.rs/hyper/latest/hyper/server/conn/struct.Connection.html#method.graceful_shutdown
                    this.conn.as_mut().graceful_shutdown();
                }
            }
        }

        match this.conn.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(x) => Poll::Ready(x),
        }
    }
}

// Handles the request and returns a response, for blobs and metadata files, also returns the
// content range.
async fn handle_request(
    executor: TaskExecutor<()>,
    server_stopped: Shared<futures::channel::oneshot::Receiver<()>>,
    repo_manager: Arc<RepositoryManager>,
    sse_response_creators: Arc<SseResponseCreatorMap>,
    req: Request<hyper::body::Incoming>,
) -> (Response<Body>, Option<ContentRange>) {
    let mut path = req.uri().path();

    // Ignore the leading slash.
    if path.starts_with('/') {
        path = &path[1..];
    }

    if path.is_empty() {
        return (generate_repository_html(repo_manager), None);
    }

    let (repo_name, resource_path) = path.split_once('/').unwrap_or((path, ""));

    let status_response = |status_code| (status_response(status_code), None);
    let Some(repo) = repo_manager.get(repo_name) else {
        return status_response(StatusCode::NOT_FOUND);
    };

    if resource_path.is_empty() {
        return (generate_package_html(repo, repo_name).await, None);
    }

    let headers = req.headers();
    let range = if let Some(header) = headers.get(RANGE) {
        if let Ok(range) = Range::from_http_range_header(header) {
            range
        } else {
            return status_response(StatusCode::RANGE_NOT_SATISFIABLE);
        }
    } else {
        Range::Full
    };

    let resource = match resource_path {
        "auto" => {
            if repo.read().await.supports_watch() {
                return (
                    handle_auto(executor, server_stopped, repo_name, repo, sse_response_creators)
                        .await,
                    None,
                );
            } else {
                // The repo doesn't support watching.
                return status_response(StatusCode::NOT_FOUND);
            }
        }
        // Enable config retrieval over HTTP to support backwards-compatible repository registration.
        "repo.config" => {
            // Retrieve "host" ssh connection from request header.
            let host_ssh_connection = match headers.get("host") {
                Some(h) => h.to_str().unwrap(),
                _ => {
                    error!("failed to find host header in headers list");
                    return status_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let mirror_url =
                match Uri::try_from(format!("http://{host_ssh_connection}/{repo_name}")) {
                    Ok(m) => m,
                    Err(e) => {
                        error!("invalid mirror uri conversion: {:?}", e);
                        return status_response(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                };

            let repo_url = match RepositoryUrl::parse(format!("fuchsia-pkg://{repo_name}").as_str())
            {
                Ok(r) => r,
                Err(e) => {
                    error!("invalid repo url conversion: {:?}", e);
                    return status_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let config = match repo.read().await.get_config(repo_url, mirror_url, None) {
                Ok(c) => c,
                Err(e) => {
                    error!("failed to generate config: {:?}", e);
                    return status_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            match config.try_into() {
                Ok(c) => c,
                Err(e) => {
                    error!("failed to generate config: {:?}", e);
                    return status_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
        _ => {
            let res = if let Some(resource_path) = resource_path.strip_prefix("blobs/") {
                repo.read().await.fetch_blob_range(resource_path, range).await
            } else {
                repo.read().await.fetch_metadata_range(resource_path, range).await
            };

            match res {
                Ok(file) => file,
                Err(RepoError::NotFound) => {
                    warn!("could not find resource: {}", resource_path);
                    return status_response(StatusCode::NOT_FOUND);
                }
                Err(RepoError::InvalidPath(path)) => {
                    warn!("invalid path: {}", path);
                    return status_response(StatusCode::BAD_REQUEST);
                }
                Err(RepoError::RangeNotSatisfiable) => {
                    warn!("invalid range: {:?}", range);
                    return status_response(StatusCode::RANGE_NOT_SATISFIABLE);
                }
                Err(err) => {
                    error!("error fetching file {}: {:?}", resource_path, err);
                    return status_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    };

    // Send the response back to the caller.
    let mut builder = Response::builder()
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", resource.content_len());

    // If we requested a subset of the file, respond with the partial content headers.
    builder = if let Some(content_range) = resource.content_range.to_http_content_range_header() {
        builder.header("Content-Range", content_range).status(StatusCode::PARTIAL_CONTENT)
    } else {
        builder.status(StatusCode::OK)
    };

    (builder.body(Body::wrap_stream(resource.stream)).unwrap(), Some(resource.content_range))
}

fn generate_repository_html(repo_manager: Arc<RepositoryManager>) -> Response<Body> {
    let middle = repo_manager
        .repositories()
        .map(|(name, _)| Ok(format!("<li><a href=\"{name}\">{name}</a></li>")))
        .collect::<Vec<Result<String, std::io::Error>>>();

    let chunks = std::iter::once(Ok(REPOSITORY_PREFIX.to_string()))
        .chain(middle)
        .chain(std::iter::once(Ok(REPOSITORY_SUFFIX.to_string())));
    let body = Body::wrap_stream(futures::stream::iter(chunks));

    Response::builder()
        .header("Content-Type", "text/html; charset=utf-8")
        .status(StatusCode::OK)
        .body(body)
        .unwrap()
}

async fn generate_package_html(
    repo: Arc<async_lock::RwLock<RepoClient<Box<dyn RepoProvider>>>>,
    repo_name: &str,
) -> Response<Body> {
    // Update the repository.
    if repo.write().await.update().await.is_err() {
        log::error!("Unable to update repository {}", repo_name);
        return status_response(StatusCode::BAD_REQUEST);
    }

    match repo.read().await.list_packages().await {
        Err(_) => status_response(StatusCode::BAD_REQUEST),
        Ok(mut packages) => {
            packages.sort_by(|a, b| a.name.partial_cmp(&b.name).unwrap());

            let middle = packages
                .into_iter()
                .map(|p| {
                    Ok(format!(
                        "<tr>
            <td><a href=\"fuchsia-pkg://fuchsia-pkg://{0}/{1}\">{1}</a></td>
            <td class=merkle>{2}</td>
          </tr>",
                        repo_name, p.name, p.hash,
                    ))
                })
                .collect::<Vec<Result<String, std::io::Error>>>();

            let chunks = std::iter::once(Ok::<String, std::io::Error>(PACKAGE_PREFIX.to_string()))
                .chain(middle)
                .chain(std::iter::once(Ok(PACKAGE_SUFFIX.to_string())));

            let body = Body::wrap_stream(futures::stream::iter(chunks));

            Response::builder()
                .header("Content-Type", "text/html; charset=utf-8")
                .status(StatusCode::OK)
                .body(body)
                .unwrap()
        }
    }
}

async fn handle_auto(
    executor: TaskExecutor<()>,
    mut server_stopped: Shared<futures::channel::oneshot::Receiver<()>>,
    repo_name: &str,
    repo: Arc<AsyncRwLock<RepoClient<Box<dyn RepoProvider>>>>,
    sse_response_creators: Arc<SseResponseCreatorMap>,
) -> Response<Body> {
    let response_creator = sse_response_creators.read().unwrap().get(repo_name).map(Arc::clone);

    // Exit early if we've already created an auto-handler.
    if let Some(response_creator) = response_creator {
        return response_creator.create().await.map(Body::from);
    }

    // Otherwise, create a timestamp watch stream. We'll do it racily to avoid holding the lock and
    // blocking the executor.
    let watcher = match repo.read().await.watch() {
        Ok(watcher) => watcher,
        Err(err) => {
            warn!("error creating file watcher: {}", err);
            return status_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Next, create a response creator. It's possible we raced another call, which could have
    // already created a creator for us. This is denoted by `sender` being `None`.
    let (response_creator, sender) = {
        let mut sse_response_creators = sse_response_creators.write().unwrap();

        if let Some(response_creator) = sse_response_creators.get(repo_name) {
            (Arc::clone(response_creator), None)
        } else {
            // Next, create a response creator.
            let (response_creator, sender) =
                SseResponseCreator::with_additional_buffer_size(AUTO_BUFFER_SIZE);

            let response_creator = Arc::new(response_creator);
            sse_response_creators.insert(repo_name.to_owned(), Arc::clone(&response_creator));

            (response_creator, Some(sender))
        }
    };

    // Spawn the watcher if one doesn't exist already. This will run in the background, and register
    // a drop callback that will shut down the watcher when the repository is closed.
    if let Some(sender) = sender {
        // Make sure the entry is cleaned up if the repository is deleted.
        let sse_response_creators = Arc::downgrade(&sse_response_creators);

        // Grab a handle to the repo dropped signal, so we can shut down our watcher.
        let mut repo_dropped = repo.read().await.on_dropped_signal();

        // Downgrade our repository handle, so we won't block it being deleted.
        let repo_name = repo_name.to_owned();
        let repo = Arc::downgrade(&repo);

        executor.spawn(async move {
            let watcher_fut = timestamp_watcher(repo, sender, watcher).fuse();
            futures::pin_mut!(watcher_fut);

            // Run the task until the watcher exits, or we were asked to cancel.
            futures::select! {
                () = watcher_fut => {},
                _ = server_stopped => {},
                _ = repo_dropped => (),
            };

            // Clean up our SSE creators.
            if let Some(sse_response_creators) = sse_response_creators.upgrade() {
                sse_response_creators.write().unwrap().remove(&repo_name);
            }
        });
    };

    // Finally, create the response for the client.
    response_creator.create().await.map(Body::from)
}

#[derive(Serialize, Deserialize)]
struct SignedTimestamp {
    signed: TimestampFile,
}
#[derive(Serialize, Deserialize)]
struct TimestampFile {
    version: u32,
}

async fn timestamp_watcher<S>(
    repo: Weak<AsyncRwLock<RepoClient<Box<dyn RepoProvider>>>>,
    sender: EventSender,
    mut watcher: S,
) where
    S: Stream<Item = ()> + Unpin,
{
    let mut old_version = None;

    loop {
        // Temporarily upgrade the repository while we look up the timestamp.json's version.
        let version = match repo.upgrade() {
            Some(repo) => read_timestamp_version(repo).await,
            None => {
                // Exit our watcher if the repository has been deleted.
                return;
            }
        };

        if let Some(version) = version
            && old_version != Some(version)
        {
            old_version = Some(version);

            sender
                .send(
                    &Event::from_type_and_data("timestamp.json", version.to_string())
                        .expect("Could not assemble timestamp event"),
                )
                .await;
        }

        // Exit the loop if the notify watcher has shut down.
        if watcher.next().await.is_none() {
            break;
        }
    }
}

// Try to read the timestamp.json's version from the repository, or return `None` if we experience
// any errors.
async fn read_timestamp_version(
    repo: Arc<AsyncRwLock<RepoClient<Box<dyn RepoProvider>>>>,
) -> Option<u32> {
    for _ in 0..MAX_PARSE_RETRIES {
        // Read the timestamp file.
        //
        // FIXME: We should be using the TUF client to get the latest
        // timestamp in order to make sure the metadata is valid.
        match repo.read().await.fetch_metadata("timestamp.json").await {
            Ok(mut file) => {
                let mut bytes = vec![];
                match file.read_to_end(&mut bytes).await {
                    Ok(()) => match serde_json::from_slice::<SignedTimestamp>(&bytes) {
                        Ok(timestamp_file) => {
                            return Some(timestamp_file.signed.version);
                        }
                        Err(err) => {
                            warn!("failed to parse timestamp.json: {:#?}", err);
                        }
                    },
                    Err(err) => {
                        warn!("failed to read timestamp.json: {:#}", err);
                    }
                }
            }
            Err(err) => {
                warn!("failed to read timestamp.json: {:#?}", err);
            }
        };

        // We might see the file change when it's half-written, so we need to retry
        // the parse if it fails.
        fasync::Timer::new(PARSE_RETRY_DELAY).await;
    }

    // Failed to parse out the timestamp file.
    error!("failed to read timestamp.json after {} attempts", MAX_PARSE_RETRIES);

    None
}

fn status_response(status_code: StatusCode) -> Response<Body> {
    Response::builder().status(status_code).body(Body::empty()).unwrap()
}
#[derive(Derivative)]
#[derivative(Debug)]
pub struct TcpStreamWithPeerAddr {
    #[derivative(Debug = "ignore")]
    stream: TokioAsyncWrapper<TcpStream>,
    peer_addr: std::io::Result<SocketAddr>,
}

impl From<TcpStream> for TcpStreamWithPeerAddr {
    fn from(value: TcpStream) -> Self {
        Self { peer_addr: value.peer_addr(), stream: value.into_futures_stream() }
    }
}

/// Adapt [async_net::TcpStream] to work with hyper.
#[derive(Debug)]
pub enum ConnectionStream {
    Tcp(TcpStreamWithPeerAddr),
    TargetTcp(ffx_target_net::TargetTcpStream),
}

impl tokio::io::AsyncRead for ConnectionStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            ConnectionStream::Tcp(t) => {
                Pin::new(&mut t.stream).poll_read(cx, buf.initialize_unfilled())
            }
            ConnectionStream::TargetTcp(t) => {
                futures::AsyncRead::poll_read(Pin::new(t), cx, buf.initialize_unfilled())
            }
        }
        .map_ok(|sz| {
            buf.advance(sz);
        })
    }
}

impl tokio::io::AsyncWrite for ConnectionStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            ConnectionStream::Tcp(t) => Pin::new(&mut t.stream).poll_write(cx, buf),
            ConnectionStream::TargetTcp(t) => futures::AsyncWrite::poll_write(Pin::new(t), cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            ConnectionStream::Tcp(t) => Pin::new(&mut t.stream).poll_flush(cx),
            ConnectionStream::TargetTcp(t) => futures::AsyncWrite::poll_flush(Pin::new(t), cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            ConnectionStream::Tcp(t) => Pin::new(&mut t.stream).poll_close(cx),
            ConnectionStream::TargetTcp(t) => Pin::new(t).poll_close(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        make_file_system_repository, make_writable_empty_repository, repo_key,
    };
    use assert_matches::assert_matches;
    use bytes::Bytes;
    use camino::Utf8Path;
    use fidl_fuchsia_pkg_ext::{MirrorConfigBuilder, RepositoryConfig, RepositoryConfigBuilder};
    use fuchsia_async as fasync;
    use fuchsia_hyper::HttpClient;
    use http_body_util::BodyExt;
    use http_sse::Client as SseClient;
    use std::fs::remove_file;
    use std::io::Write as _;
    use std::net::Ipv4Addr;

    /// Make a GET request to some `url`.
    ///
    /// This takes a `client`, which allows hyper to do connection pooling.
    async fn get(
        client: &HttpClient,
        url: impl AsRef<str>,
    ) -> Result<Response<hyper::body::Incoming>> {
        let req = Request::get(url.as_ref()).body(http_body_util::Full::default())?;
        let response = client.request(req).await?;
        Ok(response)
    }

    /// Make a GET request to some `url`, which will collect the response into a single `Bytes`
    /// chunk.
    ///
    /// This takes a `client`, which allows hyper to do connection pooling.
    async fn get_bytes(
        client: &HttpClient,
        url: impl AsRef<str> + std::fmt::Debug,
    ) -> Result<Bytes> {
        let response = get(client, url).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["Accept-Ranges"], "bytes");
        Ok(response.into_body().collect().await?.to_bytes())
    }

    /// Make a GET request to some `url` for some range of bytes.
    ///
    /// This takes a `client`, which allows hyper to do connection pooling.
    async fn get_range(
        client: &HttpClient,
        url: impl AsRef<str>,
        start: Option<u64>,
        end: Option<u64>,
    ) -> Result<Response<hyper::body::Incoming>> {
        let start_str = match start {
            Some(start) => start.to_string(),
            None => "".to_owned(),
        };
        let end_str = match end {
            Some(end) => end.to_string(),
            None => "".to_owned(),
        };
        let req = Request::get(url.as_ref())
            .header("Range", format!("bytes={start_str}-{end_str}"))
            .body(http_body_util::Full::default())?;
        let response = client.request(req).await?;
        Ok(response)
    }

    /// Make a GET request to some `url` for some range of bytes, which will collect the response
    /// into a single `Bytes` chunk.
    ///
    /// This takes a `client`, which allows hyper to do connection pooling.
    async fn get_bytes_range(
        client: &HttpClient,
        url: impl AsRef<str> + std::fmt::Debug,
        start: Option<u64>,
        end: Option<u64>,
        total_len: u64,
    ) -> Result<Bytes> {
        let response = get_range(client, url, start, end).await?;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);

        // http ranges are inclusive, so we need to add one to `end` to compute the content length.
        let content_len = end.map(|end| end + 1).unwrap_or(total_len) - start.unwrap_or(0);
        assert_eq!(response.headers()["Content-Length"], content_len.to_string());

        let start_str = start.map(|i| i.to_string()).unwrap_or_default();
        let end_str = end.map(|i| i.to_string()).unwrap_or_default();
        assert_eq!(
            response.headers()["Content-Range"],
            format!("bytes {start_str}-{end_str}/{total_len}")
        );

        Ok(response.into_body().collect().await?.to_bytes())
    }

    fn write_file(path: &Utf8Path, body: &[u8]) {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(body).unwrap();
        tmp.persist(path).unwrap();
    }

    async fn verify_repo_config(client: &HttpClient, devhost: &str, server_url: &str) {
        let config: RepositoryConfig = serde_json::from_slice(
            &get_bytes(client, &format!("{server_url}/{devhost}/repo.config")).await.unwrap(),
        )
        .unwrap();

        let expected = RepositoryConfigBuilder::new(
            RepositoryUrl::parse(&format!("fuchsia-pkg://{devhost}")).unwrap(),
        )
        .add_mirror(
            MirrorConfigBuilder::new(Uri::try_from(&format!("{server_url}/{devhost}")).unwrap())
                .unwrap()
                .subscribe(true)
                .build(),
        )
        .add_root_key(repo_key())
        .build();

        assert_eq!(config, expected);
    }

    async fn run_test<F, R>(manager: Arc<RepositoryManager>, test: F)
    where
        F: Fn(String) -> R,
        R: Future<Output = ()>,
    {
        let addr = (Ipv4Addr::LOCALHOST, 0).into();
        let (server_fut, _, server) =
            RepositoryServer::builder(addr, Arc::clone(&manager)).start().await.unwrap();

        // Run the server in the background.
        let task = fasync::Task::local(server_fut);

        test(server.local_url()).await;

        // Signal the server to shutdown.
        server.stop();

        // Wait for the server to actually shut down.
        task.await;
    }

    #[fuchsia::test]
    async fn test_start_stop() {
        let manager = RepositoryManager::new();
        let addr = (Ipv4Addr::LOCALHOST, 0).into();
        let (server_fut, _, server) =
            RepositoryServer::builder(addr, Arc::clone(&manager)).start().await.unwrap();

        // Run the server in the background.
        let task = fasync::Task::local(server_fut);

        // Signal the server to shutdown.
        server.stop();

        // Wait for the server to actually shut down.
        task.await;
    }

    #[fuchsia::test]
    async fn test_empty_server() {
        let client = fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        run_test(manager, |server_url| async {
            let result = get(&client, server_url).await.unwrap();
            assert_eq!(result.status(), StatusCode::OK);
            let body = format!("{REPOSITORY_PREFIX}{REPOSITORY_SUFFIX}");
            let body_bytes = body.as_bytes();
            let result_body_bytes = result.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(result_body_bytes, body_bytes);
        })
        .await
    }

    #[fuchsia::test]
    async fn test_get_repositories_from_empty_path() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let test_cases = [("devhost-0", ["0-0", "0-1"]), ("devhost-1", ["1-0", "1-1"])];

        for (devhost, bodies) in &test_cases {
            let dir = dir.join(devhost);
            let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

            for body in &bodies[..] {
                write_file(&dir.join("repository").join(body), body.as_bytes());
            }

            manager.add(*devhost, repo);
        }

        run_test(manager, |server_url| async move {
            let result = get(client, server_url).await.unwrap();
            assert_eq!(result.status(), StatusCode::OK);
            let result_body_bytes = result.into_body().collect().await.unwrap().to_bytes();

            let middle = "<li><a href=\"devhost-0\">devhost-0</a></li><li><a href=\"devhost-1\">devhost-1</a></li>".to_string();
            let body_str = format!("{REPOSITORY_PREFIX}{middle}{REPOSITORY_SUFFIX}");
            let body_bytes = body_str.as_bytes();
            assert_eq!(result_body_bytes, body_bytes);
        })
        .await
    }

    #[fuchsia::test]
    async fn test_get_packages_from_empty_resource_path() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let devhost = "devhost-0";
        let src_repo_dir = dir.join(devhost);

        let src_metadata_dir = src_repo_dir.join("metadata");
        let src_blobs_dir = src_repo_dir.join("blobs");
        let repo = make_file_system_repository(&src_metadata_dir, &src_blobs_dir).await;

        manager.add(devhost, repo);

        run_test(manager, |server_url| async move {
            let url =  format!("{server_url}/{devhost}");
            let result = get(client, url).await.unwrap();
            assert_eq!(result.status(), StatusCode::OK);
            let result_body_bytes = result.into_body().collect().await.unwrap().to_bytes();

            let middle = "<tr>\n            \
            <td><a href=\"fuchsia-pkg://fuchsia-pkg://devhost-0/package1/0\">package1/0</a></td>\n            \
            <td class=merkle>2881455493b5870aaea36537d70a2adc635f516ac2092598f4b6056dabc6b25d</td>\n          \
            </tr><tr>\n            \
            <td><a href=\"fuchsia-pkg://fuchsia-pkg://devhost-0/package2/0\">package2/0</a></td>\n            \
            <td class=merkle>050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458</td>\n          \
            </tr>".to_string();

            let body_str = format!("{PACKAGE_PREFIX}{middle}{PACKAGE_SUFFIX}");

            let body_bytes = body_str.as_bytes();
            assert_eq!(result_body_bytes, body_bytes);
        })
        .await
    }

    #[fuchsia::test]
    async fn test_empty_packages_from_empty_resource_path() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let test_cases = ["devhost-0", "devhost-1"];

        for devhost in &test_cases {
            let dir = dir.join(devhost);
            let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

            manager.add(*devhost, repo);
        }

        run_test(manager, |server_url| async move {
            for devhost in &test_cases {
                let url = format!("{server_url}/{devhost}");
                let result = get(client, url).await.unwrap();
                assert_eq!(result.status(), StatusCode::OK);
                let result_body_bytes = result.into_body().collect().await.unwrap().to_bytes();

                let body_str = format!("{PACKAGE_PREFIX}{PACKAGE_SUFFIX}");
                let body_bytes = body_str.as_bytes();
                assert_eq!(result_body_bytes, body_bytes);
            }
        })
        .await
    }

    #[fuchsia::test]
    async fn test_get_404_from_repositories() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let devhost = "right_repo";
        let dir = dir.join(devhost);
        let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

        manager.add(devhost, repo);

        run_test(manager, |server_url| async move {
            let url = format!("{server_url}/wrong_repo_name");
            let result = get(client, url).await.unwrap();
            assert_eq!(result.status(), StatusCode::NOT_FOUND);
        })
        .await
    }

    #[fuchsia::test]
    async fn test_get_files_from_repositories() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let test_cases = [("devhost-0", ["0-0", "0-1"]), ("devhost-1", ["1-0", "1-1"])];

        for (devhost, bodies) in &test_cases {
            let dir = dir.join(devhost);
            let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

            for body in &bodies[..] {
                write_file(&dir.join("repository").join(body), body.as_bytes());
            }

            manager.add(*devhost, repo);
        }

        run_test(manager, |server_url| async move {
            for (devhost, bodies) in &test_cases {
                for body in &bodies[..] {
                    let url = format!("{server_url}/{devhost}/{body}");
                    assert_matches!(get_bytes(client, &url).await, Ok(bytes) if bytes == body[..]);
                }
            }
        })
        .await
    }

    #[fuchsia::test]
    async fn test_get_files_with_close_range_from_repositories() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let test_cases = [("devhost-0", ["0-0", "0-1"]), ("devhost-1", ["1-0", "1-1"])];

        for (devhost, bodies) in &test_cases {
            let dir = dir.join(devhost);
            let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

            for body in &bodies[..] {
                write_file(&dir.join("repository").join(body), body.as_bytes());
            }

            manager.add(*devhost, repo);
        }

        run_test(manager, |server_url| async move {
            for (devhost, bodies) in &test_cases {
                for body in &bodies[..] {
                    let url = format!("{server_url}/{devhost}/{body}");

                    assert_eq!(
                        &body[1..=2],
                        get_bytes_range(
                            client,
                            &url,
                            Some(1),
                            Some(2),
                            body.chars().count().try_into().unwrap(),
                        )
                        .await
                        .unwrap()
                    );
                }
                verify_repo_config(client, devhost, &server_url).await;
            }
        })
        .await
    }

    #[fuchsia::test]
    async fn test_get_files_with_get_416_when_range_too_big() {
        let client = &fuchsia_hyper::new_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let test_cases = [("devhost-0", ["0-0", "0-1"]), ("devhost-1", ["1-0", "1-1"])];

        for (devhost, bodies) in &test_cases {
            let dir = dir.join(devhost);
            let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

            for body in &bodies[..] {
                write_file(&dir.join("repository").join(body), body.as_bytes());
            }

            manager.add(*devhost, repo);
        }

        run_test(manager, |server_url| async move {
            for (devhost, bodies) in &test_cases {
                for body in &bodies[..] {
                    let url = format!("{server_url}/{devhost}/{body}");
                    let response = get_range(client, &url, Some(1), Some(5)).await.unwrap();
                    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
                }
            }
        })
        .await
    }

    #[fuchsia::test]
    async fn test_auto_inner() {
        let client = &fuchsia_hyper::new_https_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().join("devhost");

        let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

        let timestamp_file = dir.join("repository").join("timestamp.json");
        write_file(
            &timestamp_file,
            serde_json::to_string(&SignedTimestamp { signed: TimestampFile { version: 1 } })
                .unwrap()
                .as_bytes(),
        );

        manager.add("devhost", repo);

        run_test(manager, |server_url| {
            let timestamp_file = timestamp_file.clone();
            async move {
                let url = format!("{server_url}/devhost/auto");
                let mut sse_client =
                    SseClient::from_hyper_client(client, url).await.unwrap().fuse();

                futures::select! {
                    value = sse_client.next() => {
                        assert_eq!(value.unwrap().unwrap().data(), "1");
                    },
                    () = fuchsia_async::Timer::new(Duration::from_secs(3)).fuse() => {
                        panic!("timed out reading auto");
                    },
                }

                write_file(
                    &timestamp_file,
                    serde_json::to_string(&SignedTimestamp {
                        signed: TimestampFile { version: 2 },
                    })
                    .unwrap()
                    .as_bytes(),
                );
                assert_eq!(sse_client.next().await.unwrap().unwrap().data(), "2");

                write_file(
                    &timestamp_file,
                    serde_json::to_string(&SignedTimestamp {
                        signed: TimestampFile { version: 3 },
                    })
                    .unwrap()
                    .as_bytes(),
                );
                assert_eq!(sse_client.next().await.unwrap().unwrap().data(), "3");

                remove_file(&timestamp_file).unwrap();
                write_file(
                    &timestamp_file,
                    serde_json::to_string(&SignedTimestamp {
                        signed: TimestampFile { version: 4 },
                    })
                    .unwrap()
                    .as_bytes(),
                );
                assert_eq!(sse_client.next().await.unwrap().unwrap().data(), "4");
            }
        })
        .await;

        // FIXME(https://github.com/notify-rs/notify/pull/337): On OSX, notify uses a
        // crossbeam-channel in `Drop` to shut down the interior thread. Unfortunately this can
        // trip over an issue where OSX will tear down the thread local storage before shutting
        // down the thread, which can trigger a panic. To avoid this issue, sleep a little bit
        // after shutting down our stream.
        fasync::Timer::new(Duration::from_millis(100)).await;
    }

    #[fuchsia::test]
    async fn test_shutdown_timeout() {
        let client = fuchsia_hyper::new_https_client();
        let manager = RepositoryManager::new();

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().join("devhost");

        let repo = make_writable_empty_repository(dir.clone()).await.unwrap();

        let timestamp_file = dir.join("repository").join("timestamp.json");
        write_file(
            &timestamp_file,
            serde_json::to_string(&SignedTimestamp { signed: TimestampFile { version: 1 } })
                .unwrap()
                .as_bytes(),
        );

        manager.add("devhost", repo);

        let addr = (Ipv4Addr::LOCALHOST, 0).into();
        let (server_fut, _, server) =
            RepositoryServer::builder(addr, Arc::clone(&manager)).start().await.unwrap();

        // Run the server in the background.
        let server_task = fasync::Task::local(server_fut);

        // Connect to the sse endpoint.
        let url = format!("{}/devhost/auto", server.local_url());

        // wait for an SSE event in the background.
        let (tx_sse_connected, rx_sse_connected) = futures::channel::oneshot::channel();
        let sse_task = fasync::Task::local(async move {
            let mut sse = SseClient::from_hyper_client(&client, url).await.unwrap();

            // We should receive one event for the current timestamp.
            sse.next().await.unwrap().unwrap();

            tx_sse_connected.send(()).unwrap();

            // We should block until we receive an error because the server went away.
            match sse.next().await {
                Some(Ok(event)) => {
                    panic!("unexpected event {event:?}");
                }
                Some(Err(_)) => {}
                None => {
                    panic!("unexpected channel close");
                }
            }
        });

        // wait for the sse client to connect to the server.
        rx_sse_connected.await.unwrap();

        // Signal the server to shutdown.
        server.stop();

        // The server should shutdown after the timeout period.
        server_task.await;

        futures::select! {
            () = sse_task.fuse() => {},

            () = fuchsia_async::Timer::new(Duration::from_secs(10)).fuse() => {
                panic!("sse task failed to shut down");
            },
        }
    }
}
