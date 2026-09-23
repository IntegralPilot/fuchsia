// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tools to support working with Fuchsia package repositories hosted on Google Cloud storage.
//!
//! See
//! - [Package](https://fuchsia.dev/fuchsia-src/concepts/packages/package?hl=en)
//! - [TUF](https://theupdateframework.io/)

use crate::range::{ContentLength, Range};
use crate::repository::{Error, RepoProvider, RepositorySpec};
use crate::resource::Resource;
use crate::util::file_stream;
use anyhow::{Context as _, anyhow};
use futures::future::BoxFuture;
use futures::{AsyncRead, FutureExt as _, TryStreamExt as _};
use http_body_util::{BodyExt, BodyStream};
use hyper::header::CONTENT_LENGTH;
use hyper::{Response, StatusCode};
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::io::{self, Seek as _, SeekFrom, Write as _};
use std::time::SystemTime;
use tempfile::SpooledTempFile;
use tuf::metadata::{MetadataPath, MetadataVersion, TargetPath};
use tuf::pouf::Pouf1;
use tuf::repository::RepositoryProvider as TufRepositoryProvider;
use url::Url;

const UNKNOWN_CONTENT_LEN_BUF_SIZE: usize = 8_196;

/// Helper trait that lets us mock gcs::client::Client for testing.
#[doc(hidden)]
#[async_trait::async_trait]
pub trait GcsClient<B = gcs::client::ResponseBody> {
    async fn stream(&self, bucket: &str, object: &str) -> anyhow::Result<Response<B>>;
}

#[async_trait::async_trait]
impl GcsClient<gcs::client::ResponseBody> for gcs::client::Client {
    async fn stream(
        &self,
        bucket: &str,
        object: &str,
    ) -> anyhow::Result<Response<gcs::client::ResponseBody>> {
        gcs::client::Client::stream(self, bucket, object).await
    }
}

/// [GcsRepository] serves a package repository from a Google Cloud Storage bucket.
pub struct GcsRepository<T = gcs::client::Client, B = gcs::client::ResponseBody> {
    client: T,

    /// URL to the GCS bucket and object prefix that contains the metadata repository. This must
    /// have the gs:// URL scheme. The constructor will make sure this has a trailing slash so it
    /// is treated as a directory.
    metadata_repo_url: Url,

    /// URL to the GCS bucket and object prefix that contains the blobs repository. This must have
    /// the gs:// URL scheme. The constructor will make sure this has a trailing slash so it is
    /// treated as a directory.
    blob_repo_url: Url,
    aliases: BTreeSet<String>,
    _phantom: std::marker::PhantomData<fn() -> B>,
}

impl<T: Debug, B> Debug for GcsRepository<T, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsRepository")
            .field("client", &self.client)
            .field("metadata_repo_url", &self.metadata_repo_url)
            .field("blob_repo_url", &self.blob_repo_url)
            .field("aliases", &self.aliases)
            .finish()
    }
}

impl<T, B> GcsRepository<T, B>
where
    T: GcsClient<B> + Debug + Send + Sync,
    B: hyper::body::Body<Data = hyper::body::Bytes> + Send + Sync + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    pub fn new(
        client: T,
        mut metadata_repo_url: Url,
        mut blob_repo_url: Url,
    ) -> Result<Self, anyhow::Error> {
        if metadata_repo_url.scheme() != "gs" {
            return Err(anyhow!("unsupported scheme {}", metadata_repo_url));
        }

        if blob_repo_url.scheme() != "gs" {
            return Err(anyhow!("unsupported scheme {}", blob_repo_url));
        }

        // `Url::join()` treats urls with a trailing slash as a directory, and without as a file. In
        // the latter case, it will strip off the last segment before joining paths. Since the
        // metadata and blob url are directories, make sure they have a trailing slash.
        if !metadata_repo_url.path().ends_with('/') {
            metadata_repo_url.set_path(&format!("{}/", metadata_repo_url.path()));
        }

        if !blob_repo_url.path().ends_with('/') {
            blob_repo_url.set_path(&format!("{}/", blob_repo_url.path()));
        }

        Ok(Self {
            client,
            metadata_repo_url,
            blob_repo_url,
            aliases: BTreeSet::new(),
            _phantom: std::marker::PhantomData,
        })
    }

    fn get<'a>(
        &'a self,
        root: &Url,
        path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        let url = root.join(path);

        async move {
            let url = url.map_err(|err| anyhow!(err))?;
            let bucket =
                url.host_str().ok_or_else(|| anyhow!("url must include a bucket: {}", url))?;
            let object = url.path();

            // FIXME(https://fxbug.dev/42181385): The gcs library does not yet support range requests, so
            // always fetch the full range.
            let resp = self.client.stream(bucket, object).await?;

            match resp.status() {
                StatusCode::OK => {
                    // `Resource` requires us to know the exact length of the artifact. That's the case
                    // if we get a `Content-Length` header.
                    if let Some(content_len) = resp.headers().get(CONTENT_LENGTH) {
                        let content_len =
                            ContentLength::from_http_content_length_header(content_len)
                                .with_context(|| format!("parsing Content-Length header: {url}"))?;

                        // Make sure we didn't try to fetch data that's out of bounds.
                        if !content_len.contains_range(range) {
                            return Err(Error::RangeNotSatisfiable);
                        }

                        let body = resp.into_body();

                        return Ok(Resource {
                            content_range: content_len.into(),
                            stream: Box::pin(
                                BodyStream::new(body)
                                    .map_ok(|frame| frame.into_data().unwrap_or_default())
                                    .map_err(std::io::Error::other),
                            ),
                        });
                    }

                    // If we didn't get a `Content-Length` header (e.g. dynamic transcoding
                    // or chunked transfer encoding), buffer through a spooled temp file
                    // to compute the final resource length.
                    return self.get_with_stored_content_len(resp.into_body(), range).await;
                }
                StatusCode::NOT_FOUND => Err(Error::NotFound),
                StatusCode::RANGE_NOT_SATISFIABLE => Err(Error::RangeNotSatisfiable),
                status => {
                    if status.is_success() {
                        Err(Error::Other(anyhow!("unexpected status code {}: {}", url, status)))
                    } else {
                        // GCS may return a more detailed error description in the body.
                        match resp.into_body().collect().await {
                            Ok(body) => {
                                let body = body.to_bytes();
                                let body_str = String::from_utf8_lossy(&body);
                                Err(Error::Other(anyhow!(
                                    "error downloading resource {}: {}\n{}",
                                    url,
                                    status,
                                    body_str
                                )))
                            }
                            Err(_) => Err(Error::Other(anyhow!(
                                "error downloading resource {}: {}",
                                url,
                                status
                            ))),
                        }
                    }
                }
            }
        }
        .boxed()
    }

    /// Handles a response that contains a `x-goog-stored-content-length` header.
    ///
    /// Since we don't know the size of the resource, we'll buffer up to a certain size, then
    /// transition over into a temporary file. Once the file has been fully downloaded, we will
    /// compute the actual length, then return it.
    async fn get_with_stored_content_len(
        &self,
        mut body: B,
        range: Range,
    ) -> Result<Resource, Error> {
        let mut file = SpooledTempFile::new(UNKNOWN_CONTENT_LEN_BUF_SIZE);

        let mut total_len = 0;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| Error::Other(anyhow!(e)))?;
            if let Ok(chunk) = frame.into_data() {
                total_len += chunk.len() as u64;
                file.write_all(&chunk).map_err(Error::Io)?;
            }
        }
        file.flush().map_err(Error::Io)?;

        file.seek(SeekFrom::Start(0)).map_err(Error::Io)?;

        // Make sure we didn't try to fetch data that's out of bounds.
        let content_len = ContentLength::new(total_len);
        if !content_len.contains_range(range) {
            return Err(Error::RangeNotSatisfiable);
        }

        Ok(Resource {
            content_range: content_len.into(),
            stream: Box::pin(file_stream(total_len, file, None)),
        })
    }
}

#[async_trait::async_trait]
impl<T, B> RepoProvider for GcsRepository<T, B>
where
    T: GcsClient<B> + Debug + Send + Sync + 'static,
    B: hyper::body::Body<Data = hyper::body::Bytes> + Send + Sync + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    fn spec(&self) -> RepositorySpec {
        RepositorySpec::Gcs {
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
        self.get(&self.metadata_repo_url, resource_path, range)
    }

    fn fetch_blob_range<'a>(
        &'a self,
        resource_path: &str,
        range: Range,
    ) -> BoxFuture<'a, Result<Resource, Error>> {
        self.get(&self.blob_repo_url, resource_path, range)
    }

    fn blob_modification_time<'a>(
        &'a self,
        _path: &str,
    ) -> BoxFuture<'a, Result<Option<SystemTime>, anyhow::Error>> {
        // FIXME(https://fxbug.dev/42181387): The gcs library does not expose an API to determine the
        // blob modification time.
        async move { Ok(None) }.boxed()
    }
}

impl<T, B> TufRepositoryProvider<Pouf1> for GcsRepository<T, B>
where
    T: GcsClient<B> + Debug + Send + Sync + 'static,
    B: hyper::body::Body<Data = hyper::body::Bytes> + Send + Sync + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    fn fetch_metadata<'a>(
        &'a self,
        meta_path: &MetadataPath,
        version: MetadataVersion,
    ) -> BoxFuture<'a, Result<Box<dyn AsyncRead + Send + Unpin + 'a>, tuf::Error>> {
        let meta_path = meta_path.clone();
        let path = meta_path.components::<Pouf1>(version).join("/");

        async move {
            let resp =
                self.fetch_metadata_range(&path, Range::Full).await.map_err(|err| match err {
                    Error::Tuf(err) => err,
                    Error::NotFound => tuf::Error::MetadataNotFound { path: meta_path, version },
                    err => tuf::Error::Opaque(err.to_string()),
                })?;

            let reader = resp.stream.map_err(io::Error::other).into_async_read();

            let reader: Box<dyn AsyncRead + Send + Unpin> = Box::new(reader);
            Ok(reader)
        }
        .boxed()
    }

    fn fetch_target<'a>(
        &'a self,
        target_path: &TargetPath,
    ) -> BoxFuture<'a, Result<Box<dyn AsyncRead + Send + Unpin + 'a>, tuf::Error>> {
        let target_path = target_path.clone();
        let path = target_path.components().join("/");

        async move {
            let resp =
                self.fetch_metadata_range(&path, Range::Full).await.map_err(|err| match err {
                    Error::Tuf(err) => err,
                    Error::NotFound => tuf::Error::TargetNotFound(target_path),
                    err => tuf::Error::Opaque(err.to_string()),
                })?;

            let reader = resp.stream.map_err(io::Error::other).into_async_read();

            let reader: Box<dyn AsyncRead + Send + Unpin> = Box::new(reader);
            Ok(reader)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::FileSystemRepository;
    use crate::repository::repo_tests::{self, TestEnv as _};
    use crate::test_utils::make_repo_dir;
    use crate::util::CHUNK_SIZE;
    use assert_matches::assert_matches;
    use camino::{Utf8Path, Utf8PathBuf};
    use std::fs::File;

    const X_GOOG_STORED_CONTENT_LENGTH: &str = "x-goog-stored-content-length";

    type BoxBody = http_body_util::combinators::BoxBody<hyper::body::Bytes, hyper::Error>;

    #[derive(Debug)]
    struct MockGcsClient {
        repo: FileSystemRepository,
        content_length_header: &'static str,
    }

    #[async_trait::async_trait]
    impl GcsClient<BoxBody> for MockGcsClient {
        async fn stream(
            &self,
            bucket: &str,
            mut object: &str,
        ) -> anyhow::Result<Response<BoxBody>> {
            // The gcs library allows for leading slashes, but FileSystemRepository does not, so
            // remove it.
            if let Some(o) = object.strip_prefix('/') {
                object = o;
            }

            let res = match bucket {
                "my-tuf-repo" => self.repo.fetch_metadata_range(object, Range::Full).await,
                "my-blob-repo" => self.repo.fetch_blob_range(object, Range::Full).await,
                _ => panic!("unknown bucket {bucket:?}"),
            };

            match res {
                Ok(resource) => {
                    // We don't support range requests.
                    assert_eq!(resource.content_range.to_http_content_range_header(), None);

                    let content_len = resource.content_len();
                    let mut stream = resource.stream;
                    let mut bytes = Vec::new();
                    while let Some(frame) = stream.try_next().await? {
                        bytes.extend_from_slice(&frame);
                    }

                    Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header(self.content_length_header, content_len)
                        .body(
                            http_body_util::Full::new(bytes.into())
                                .map_err(|never| match never {})
                                .boxed(),
                        )
                        .unwrap())
                }
                Err(Error::NotFound) => Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(http_body_util::Empty::new().map_err(|never| match never {}).boxed())
                    .unwrap()),
                res => panic!("unexpected result {res:#?}"),
            }
        }
    }

    struct TestEnv {
        _tmp: tempfile::TempDir,
        metadata_repo_path: Utf8PathBuf,
        blob_repo_path: Utf8PathBuf,
        repo: GcsRepository<MockGcsClient, BoxBody>,
    }

    impl TestEnv {
        async fn new() -> Self {
            Self::with_content_length_header("Content-Length").await
        }

        async fn with_content_length_header(content_length_header: &'static str) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = Utf8Path::from_path(tmp.path()).unwrap();

            // Create a repository and serve it with the server.
            let metadata_repo_path = dir.join("tuf");
            let blob_repo_path = dir.join("blobs");
            std::fs::create_dir(&metadata_repo_path).unwrap();
            std::fs::create_dir(&blob_repo_path).unwrap();

            make_repo_dir(blob_repo_path.as_std_path(), blob_repo_path.as_std_path(), None).await;
            let remote_repo =
                FileSystemRepository::new(metadata_repo_path.clone(), blob_repo_path.clone());

            let tuf_url = "gs://my-tuf-repo/";
            let blob_url = "gs://my-blob-repo/";

            let repo = GcsRepository::new(
                MockGcsClient { repo: remote_repo, content_length_header },
                Url::parse(tuf_url).unwrap(),
                Url::parse(blob_url).unwrap(),
            )
            .unwrap();

            TestEnv { _tmp: tmp, metadata_repo_path, blob_repo_path, repo }
        }
    }

    #[async_trait::async_trait]
    impl repo_tests::TestEnv for TestEnv {
        fn supports_range(&self) -> bool {
            false
        }

        fn write_metadata(&self, path: &str, bytes: &[u8]) {
            let file_path = self.metadata_repo_path.join(path);
            let mut f = File::create(file_path).unwrap();
            f.write_all(bytes).unwrap();
        }

        fn write_blob(&self, path: &str, bytes: &[u8]) {
            let file_path = self.blob_repo_path.join(path);
            let mut f = File::create(file_path).unwrap();
            f.write_all(bytes).unwrap();
        }

        fn repo(&self) -> &dyn RepoProvider {
            &self.repo
        }
    }

    mod content_length {
        use super::*;

        repo_tests::repo_test_suite! {
            env = TestEnv::new().await;
            chunk_size = CHUNK_SIZE;
        }
    }

    mod goog_stored_content_length {
        use super::*;

        repo_tests::repo_test_suite! {
            env = TestEnv::with_content_length_header(X_GOOG_STORED_CONTENT_LENGTH).await;
            chunk_size = UNKNOWN_CONTENT_LEN_BUF_SIZE;
        }
    }

    #[fuchsia::test]
    async fn test_blob_modification_time() {
        let env = TestEnv::new().await;

        std::fs::write(env.blob_repo_path.join("empty-blob"), b"").unwrap();

        // We don't support modification time.
        assert_matches!(env.repo.blob_modification_time("empty-blob").await, Ok(None));
    }

    #[fuchsia::test]
    async fn test_watch() {
        let env = TestEnv::new().await;

        // We don't support watch.
        assert_matches!(env.repo.supports_watch(), false);
        assert!(env.repo.watch().is_err());
    }
}
