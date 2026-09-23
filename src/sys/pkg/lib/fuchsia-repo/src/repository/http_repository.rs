// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tools to download a Fuchsia package from from a TUF repository.
//! See
//! - [Package](https://fuchsia.dev/fuchsia-src/concepts/packages/package?hl=en)
//! - [TUF](https://theupdateframework.io/)

use crate::range::{ContentRange, Range};
use crate::repository::{Error, RepoProvider, RepositorySpec};
use crate::resource::Resource;
use anyhow::{Context as _, Result, anyhow};
use futures::future::BoxFuture;
use futures::{AsyncRead, FutureExt as _, TryStreamExt as _};
use hyper::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use hyper::{Method, Request, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::Connect;
type Body = http_body_util::Full<hyper::body::Bytes>;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::time::SystemTime;
use tuf::metadata::{MetadataPath, MetadataVersion, TargetPath};
use tuf::pouf::Pouf1;
use tuf::repository::{
    HttpRepository as TufHttpRepository, HttpRepositoryBuilder as TufHttpRepositoryBuilder,
    RepositoryProvider as TufRepositoryProvider,
};
use url::Url;

#[derive(Debug)]
pub struct HttpRepository<C>
where
    C: Connect + Send + Sync + 'static,
{
    client: Client<C, Body>,
    tuf_repo: TufHttpRepository<C, Pouf1>,
    metadata_repo_url: Url,
    blob_repo_url: Url,
    aliases: BTreeSet<String>,
}

impl<C> HttpRepository<C>
where
    C: Connect + Clone + Debug + Send + Sync + 'static,
{
    pub fn new(
        client: Client<C, Body>,
        mut metadata_repo_url: Url,
        mut blob_repo_url: Url,
        aliases: BTreeSet<String>,
    ) -> Self {
        // `URL.join` treats urls with a trailing slash as a directory, and without as a file.
        // In the latter case, it will strip off the last segment before joining paths. Since the
        // metadata and blob url are directories, make sure they have a trailing slash.
        if !metadata_repo_url.path().ends_with('/') {
            metadata_repo_url.set_path(&format!("{}/", metadata_repo_url.path()));
        }

        if !blob_repo_url.path().ends_with('/') {
            blob_repo_url.set_path(&format!("{}/", blob_repo_url.path()));
        }

        let tuf_repo =
            TufHttpRepositoryBuilder::<_, Pouf1>::new(metadata_repo_url.clone(), client.clone())
                .build();

        Self { client, tuf_repo, metadata_repo_url, blob_repo_url, aliases }
    }
}

impl<C> HttpRepository<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    fn fetch_from<'a>(
        &'a self,
        root: &Url,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        let full_url = root.join(resource_path);

        async move {
            let full_url = full_url.map_err(|e| anyhow!(e))?;
            let uri = full_url.as_str().parse::<Uri>().map_err(|e| anyhow!(e))?;

            let mut builder = Request::builder().method(Method::GET).uri(uri);

            // Add a 'Range' header if we're requesting a subset of the file.
            if let Some(http_range) = range.to_http_request_header() {
                builder = builder.header(RANGE, http_range);
            }

            let request = builder.body(Body::default()).context("creating http request")?;

            let resp = self
                .client
                .request(request)
                .await
                .context(format!("fetching resource {}", full_url.as_str()))?;

            // Check if the response was successful, or propagate the error.
            //
            // Note that according to [RFC-7233], it's possible for a 'Range' request from the
            // client to get a `200 OK` response and the full resource in return. Furthermore, the
            // server is allowed to return a `206 Partial Content` and a different subset
            // `Content-Range` that was requested by the `Range` request. The RFC mentions that a
            // server may do this to coalesce overlapping ranges, or separated by a gap. So we'll
            // parse these headers and pass along the response range to the caller.
            //
            // [RFC-7233]: https://datatracker.ietf.org/doc/html/rfc7233#section-4.1
            let content_range = match resp.status() {
                StatusCode::OK => {
                    // The package resolver currently requires a 'Content-Length' header, so error out
                    // if one wasn't provided.
                    let content_length = resp.headers().get(CONTENT_LENGTH).ok_or_else(|| {
                        Error::Other(anyhow!("response missing Content-Length header"))
                    })?;

                    ContentRange::from_http_content_length_header(content_length)
                        .context("parsing Content-Length header")?
                }
                StatusCode::PARTIAL_CONTENT => {
                    // According to [RFC-7233], a Partial Content status must come with a
                    // 'Content-Range' header.
                    //
                    // [RFC-7233]: https://datatracker.ietf.org/doc/html/rfc7233#section-4.1
                    let content_range = resp.headers().get(CONTENT_RANGE).ok_or_else(|| {
                        Error::Other(anyhow!(
                            "received Partial Content status, but missing Content-Range header"
                        ))
                    })?;

                    ContentRange::from_http_content_range_header(content_range)
                        .context("parsing Content-Range header")?
                }
                StatusCode::NOT_FOUND => {
                    return Err(Error::NotFound);
                }
                StatusCode::RANGE_NOT_SATISFIABLE => {
                    return Err(Error::RangeNotSatisfiable);
                }
                status => {
                    if status.is_success() {
                        return Err(Error::Other(anyhow!("unexpected status code: {}", status)));
                    } else {
                        return Err(Error::Other(anyhow!(
                            "error downloading resource: {}",
                            status
                        )));
                    }
                }
            };

            let body = http_body_util::BodyStream::new(resp.into_body())
                .map_ok(|frame| frame.into_data().unwrap_or_default())
                .map_err(std::io::Error::other);

            Ok(Resource { content_range, stream: Box::pin(body) })
        }
        .boxed()
    }
}

impl<C> RepoProvider for HttpRepository<C>
where
    C: Connect + Clone + Debug + Send + Sync + 'static,
{
    fn spec(&self) -> RepositorySpec {
        RepositorySpec::Http {
            metadata_repo_url: self.metadata_repo_url.as_str().to_owned(),
            blob_repo_url: self.blob_repo_url.as_str().to_owned(),
            aliases: self.aliases.clone(),
        }
    }

    fn aliases(&self) -> &BTreeSet<String> {
        &self.aliases
    }

    fn fetch_metadata_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.fetch_from(&self.metadata_repo_url, resource_path, range)
    }

    fn fetch_blob_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.fetch_from(&self.blob_repo_url, resource_path, range)
    }

    fn blob_modification_time<'a>(
        &'a self,
        _path: &str,
    ) -> BoxFuture<'a, Result<Option<SystemTime>>> {
        async move { Ok(None) }.boxed()
    }
}

impl<C> TufRepositoryProvider<Pouf1> for HttpRepository<C>
where
    C: Connect + Clone + Debug + Send + Sync + 'static,
{
    fn fetch_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.tuf_repo.fetch_metadata(meta_path, version)
    }

    fn fetch_target<'a>(
        &'a self,
        target_path: &TargetPath,
    ) -> BoxFuture<'a, tuf::Result<Box<dyn AsyncRead + Send + Unpin + 'a>>> {
        self.tuf_repo.fetch_target(target_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::RepositoryManager;
    use crate::repo_client::RepoClient;
    use crate::repository::repo_tests::{self, TestEnv as _};
    use crate::server::RepositoryServer;
    use crate::test_utils::make_pm_repository;
    use crate::util::CHUNK_SIZE;
    use assert_matches::assert_matches;
    use camino::Utf8Path;
    use fuchsia_async as fasync;
    use fuchsia_hyper::{HyperConnector, new_client};
    use std::fs::File;
    use std::io::Write;
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    struct TestEnv {
        tmp: tempfile::TempDir,
        repo: HttpRepository<HyperConnector>,
        server: RepositoryServer,
        task: fasync::Task<()>,
    }

    impl TestEnv {
        async fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = Utf8Path::from_path(tmp.path()).unwrap();

            // Create a repository and serve it with the server.
            let remote_backend = Box::new(make_pm_repository(&dir).await) as Box<_>;
            let remote_repo = RepoClient::from_trusted_remote(remote_backend).await.unwrap();

            let manager = RepositoryManager::new();
            manager.add("tuf", remote_repo);

            let addr = (Ipv4Addr::LOCALHOST, 0).into();
            let (server_fut, _, server) =
                RepositoryServer::builder(addr, Arc::clone(&manager)).start().await.unwrap();

            // Run the server in the background.
            let task = fasync::Task::local(server_fut);

            let tuf_url = server.local_url() + "/tuf";
            let blob_url = server.local_url() + "/tuf/blobs";

            let repo = HttpRepository::new(
                new_client(),
                Url::parse(&tuf_url).unwrap(),
                Url::parse(&blob_url).unwrap(),
                BTreeSet::new(),
            );

            TestEnv { tmp, repo, server, task }
        }
    }

    #[async_trait::async_trait]
    impl repo_tests::TestEnv for TestEnv {
        fn supports_range(&self) -> bool {
            true
        }

        fn repo(&self) -> &dyn RepoProvider {
            &self.repo
        }

        fn write_metadata(&self, path: &str, bytes: &[u8]) {
            let file_path = self.tmp.path().join("repository").join(path);
            let mut f = File::create(file_path).unwrap();
            f.write_all(bytes).unwrap();
        }

        fn write_blob(&self, path: &str, bytes: &[u8]) {
            let file_path = self.tmp.path().join("repository").join("blobs").join(path);
            let mut f = File::create(file_path).unwrap();
            f.write_all(bytes).unwrap();
        }

        async fn stop(self) {
            let TestEnv { tmp: _tmp, repo, server, task } = self;
            server.stop();

            // Explicitly drop the repo so we close all our client connections before we wait for
            // the server to shut down.
            drop(repo);

            task.await;
        }
    }

    repo_tests::repo_test_suite! {
        env = TestEnv::new().await;
        chunk_size = CHUNK_SIZE;
    }

    #[fuchsia::test]
    async fn test_blob_modification_time() {
        let env = TestEnv::new().await;

        std::fs::write(env.tmp.path().join("repository").join("blobs").join("empty-blob"), b"")
            .unwrap();

        // We don't support modification time.
        assert_matches!(env.repo.blob_modification_time("empty-blob").await, Ok(None));

        env.stop().await;
    }

    #[fuchsia::test]
    async fn test_watch() {
        let env = TestEnv::new().await;

        // We don't support watch.
        assert_matches!(env.repo.supports_watch(), false);
        assert!(env.repo.watch().is_err());

        env.stop().await;
    }
}
