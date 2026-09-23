// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::artifact::ArtifactReader;
use crate::io::ReadSeek;
use anyhow::{Context, Result, anyhow};
use fuchsia_async::TimeoutExt;
use fuchsia_merkle::Hash;
use http_body_util::BodyExt;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FetchError {
    NotFound,
    HttpError(u16),
    ParseError(String),
    NetworkError(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::NotFound => write!(f, "404 Not Found"),
            FetchError::HttpError(status) => {
                if let Ok(code) = hyper::StatusCode::from_u16(*status) {
                    write!(
                        f,
                        "HTTP Status {} ({})",
                        status,
                        code.canonical_reason().unwrap_or("Unknown")
                    )
                } else {
                    write!(f, "HTTP Status {}", status)
                }
            }
            FetchError::ParseError(msg) => write!(f, "Parse Error: {}", msg),
            FetchError::NetworkError(msg) => write!(f, "Network Error: {}", msg),
        }
    }
}

impl std::error::Error for FetchError {}

pub trait HttpFetcher: Send + Sync {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError>;
}

#[derive(Default)]
pub struct RealHttpFetcher;

impl RealHttpFetcher {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Runs an async fetch operation on a thread-local executor.
/// Thread-local storage keeps `RealHttpFetcher` thread-safe (`Send + Sync`) despite `LocalExecutor`
/// being `!Send + !Sync`, while avoiding per-request executor setup overhead.
fn run_with_cached_executor<F, Fut>(f: F) -> Fut::Output
where
    F: FnOnce(fuchsia_hyper::HttpsClient) -> Fut,
    Fut: std::future::Future,
{
    thread_local! {
        static EXECUTOR_AND_CLIENT: std::cell::RefCell<(fuchsia_async::LocalExecutor, fuchsia_hyper::HttpsClient)> = std::cell::RefCell::new({
            let executor = fuchsia_async::LocalExecutor::new();
            let client = fuchsia_hyper::new_https_client();
            (executor, client)
        });
    }

    EXECUTOR_AND_CLIENT.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let (executor, client) = &mut *borrow;

        executor.run_singlethreaded(f(client.clone()))
    })
}

impl HttpFetcher for RealHttpFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        run_with_cached_executor(|client| async move {
            let uri = url
                .parse::<hyper::Uri>()
                .map_err(|e| FetchError::ParseError(format!("Invalid URL: {}", e)))?;
            let res = async {
                client
                    .get(uri)
                    .await
                    .map_err(|e| FetchError::NetworkError(format!("Connection failed: {}", e)))
            }
            .on_timeout(Duration::from_secs(30), || {
                Err(FetchError::NetworkError("Timeout connecting to server".to_string()))
            })
            .await?;
            match res.status() {
                hyper::StatusCode::OK => {
                    let body_bytes = async {
                        res.into_body().collect().await.map(|b| b.to_bytes()).map_err(|e| {
                            FetchError::NetworkError(format!("Failed to read body: {}", e))
                        })
                    }
                    .on_timeout(Duration::from_secs(300), || {
                        Err(FetchError::NetworkError("Timeout reading body".to_string()))
                    })
                    .await?;
                    Ok(body_bytes.to_vec())
                }
                hyper::StatusCode::NOT_FOUND => Err(FetchError::NotFound),
                status => Err(FetchError::HttpError(status.as_u16())),
            }
        })
    }
}

const DEFAULT_DELIVERY_BLOB_TYPES: &[u32] = &[1];

pub struct HttpArtifactReader<F: HttpFetcher> {
    fetcher: F,
    base_url: Url,
    delivery_blob_type: Option<u32>,
    cache_dir: tempfile::TempDir,
}

impl<F: HttpFetcher> HttpArtifactReader<F> {
    pub fn new(
        fetcher: F,
        blob_server_url: String,
        delivery_blob_type: Option<u32>,
    ) -> Result<Self> {
        if !blob_server_url.ends_with('/') {
            return Err(anyhow!("Base URL for blob repository must end with trailing slash"));
        }
        let base_url = Url::parse(&blob_server_url)
            .map_err(|e| anyhow!("Invalid base URL {}: {:?}", blob_server_url, e))?;

        let cache_dir =
            tempfile::tempdir().context("Failed to create temporary cache directory")?;

        Ok(Self { fetcher, base_url, delivery_blob_type, cache_dir })
    }

    fn construct_blob_url(&self, path: &str, blob_type: u32) -> Result<Url> {
        let path_segment = format!("{}/{}", blob_type, path);
        self.base_url
            .join(&path_segment)
            .map_err(|e| anyhow!("Failed to construct URL for {}: {:?}", path, e))
    }
}

impl<F: HttpFetcher> ArtifactReader for HttpArtifactReader<F> {
    fn open(&mut self, path: &Path) -> Result<Box<dyn ReadSeek>> {
        let hash_str = path.to_str().ok_or_else(|| anyhow!("Invalid non-UTF8 path: {:?}", path))?;
        // Validate the path is a Merkle hash, but we don't need to retain it
        // since the caller (OtaVerificationCollector) is responsible for verifying the payload.
        let _ = Hash::from_str(hash_str)
            .map_err(|e| anyhow!("Invalid expected Merkle hash {}: {:?}", hash_str, e))?;

        let cache_path = self.cache_dir.path().join(hash_str);

        if cache_path.exists() {
            let file = std::fs::File::open(&cache_path)?;
            return Ok(Box::new(file));
        }

        // TODO(https://b.corp.google.com/issues/535211158): Establish a clear policy for
        // determining which delivery blob formats to use. The current format
        // selection logic is brittle when servers host multiple formats, as Scrutiny might
        // falsely verify a blob type that the target device does not actually support.
        let types_to_try: &[u32] = match &self.delivery_blob_type {
            Some(t) => std::slice::from_ref(t),
            None => DEFAULT_DELIVERY_BLOB_TYPES,
        };

        let mut last_error = None;

        for &blob_type in types_to_try {
            let url = self.construct_blob_url(hash_str, blob_type)?;
            let bytes = self.fetcher.fetch(url.as_str());

            match bytes {
                Ok(b) => {
                    // Verify that the delivery blob type specified in the parsed delivery
                    // blob header matches the `blob_type` requested in the URL.
                    if let Ok(header_type) = delivery_blob::delivery_blob_type(&b) {
                        let header_type_u32 = u32::from(header_type);
                        if header_type_u32 != blob_type {
                            return Err(anyhow!(
                                "Delivery blob type mismatch for blob {}: requested type {}, header specified type {}",
                                hash_str,
                                blob_type,
                                header_type_u32
                            ));
                        }
                    }

                    let mut file = std::fs::File::create(&cache_path)?;
                    delivery_blob::decompress_to(&b, &mut file).with_context(|| {
                        format!("Failed to decompress Type {} blob {}", blob_type, hash_str)
                    })?;

                    let file = std::fs::File::open(&cache_path)?;
                    return Ok(Box::new(file));
                }
                Err(FetchError::NotFound) => {
                    // A 404 means the server does not host the blob in this specific
                    // delivery format. We continue the loop to fallback and attempt
                    // to fetch the blob using the next available delivery type.
                    last_error = Some(anyhow::Error::new(FetchError::NotFound));
                    continue;
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e)
                        .context(format!("HTTP error fetching blob {}", hash_str)));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!(FetchError::NotFound)))
    }

    fn read_bytes(&mut self, path: &Path) -> Result<Vec<u8>> {
        let mut reader = self.open(path)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut bytes)?;
        Ok(bytes)
    }

    fn get_deps(&self) -> HashSet<PathBuf> {
        HashSet::new()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;

    const TEST_URL: &str = "http://localhost:8083/blobs/";

    fn test_blob_url(blob_type: u32, hash: &Hash) -> String {
        format!("{}{}/{}", TEST_URL, blob_type, hash)
    }

    /// A mock fetcher that uses a HashMap instead of the network.
    pub struct MockHttpFetcher {
        responses: HashMap<String, Result<Vec<u8>, FetchError>>,
    }

    impl MockHttpFetcher {
        pub fn new(responses: HashMap<String, Result<Vec<u8>, FetchError>>) -> Self {
            Self { responses }
        }
    }

    impl HttpFetcher for MockHttpFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
            match self.responses.get(url) {
                Some(Ok(bytes)) => Ok(bytes.clone()),
                Some(Err(status)) => Err(status.clone()),
                None => Err(FetchError::NotFound),
            }
        }
    }

    #[test]
    fn test_artifact_reader_missing_blob() {
        let expected_hash =
            Hash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();

        let fetcher = MockHttpFetcher::new(HashMap::new());
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        let fetch_err = err.downcast_ref::<FetchError>().unwrap();
        assert!(matches!(fetch_err, FetchError::NotFound));
    }

    #[test]
    fn test_artifact_reader_type_1_success() {
        let mut responses = HashMap::new();
        let bytes = vec![1, 2, 3, 4];
        let expected_hash = fuchsia_merkle::root_from_slice(&bytes);

        let type_1_blob = delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &bytes);

        responses.insert(test_blob_url(1, &expected_hash), Ok(type_1_blob));

        let fetcher = MockHttpFetcher::new(responses);
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bytes);
    }

    #[test]
    fn test_artifact_reader_blob_type_mismatch() {
        let mut responses = HashMap::new();
        let bytes = vec![1, 2, 3, 4];
        let expected_hash = fuchsia_merkle::root_from_slice(&bytes);

        let type_1_blob = delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &bytes);

        // Serve type 1 blob under type 2 URL path.
        responses.insert(test_blob_url(2, &expected_hash), Ok(type_1_blob));

        let fetcher = MockHttpFetcher::new(responses);
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), Some(2)).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Delivery blob type mismatch"),
            "Expected mismatch error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_artifact_reader_override_type() {
        let expected_hash =
            Hash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();

        let fetcher = MockHttpFetcher::new(HashMap::new());
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), Some(1)).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        let fetch_err = err.downcast_ref::<FetchError>().unwrap();
        assert!(matches!(fetch_err, FetchError::NotFound));
    }

    #[test]
    fn test_construct_blob_url() {
        // Missing trailing slash should fail
        let mock_fetcher = MockHttpFetcher::new(HashMap::new());
        let reader_res =
            HttpArtifactReader::new(mock_fetcher, "http://localhost:8083/blobs".to_string(), None);
        assert!(reader_res.is_err());
        assert!(reader_res.err().unwrap().to_string().contains("trailing slash"));

        let fetcher = MockHttpFetcher::new(HashMap::new());
        let reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let url = reader.construct_blob_url("abc", 1).unwrap();
        assert_eq!(url.as_str(), "http://localhost:8083/blobs/1/abc");

        let fetcher2 = MockHttpFetcher::new(HashMap::new());
        let reader2 = HttpArtifactReader::new(fetcher2, TEST_URL.to_string(), None).unwrap();
        let url = reader2.construct_blob_url("abc", 99).unwrap();
        assert_eq!(url.as_str(), "http://localhost:8083/blobs/99/abc");
    }

    #[test]
    #[ignore] // Ignored by default to avoid failing in offline CI builds
    fn test_real_http_fetcher_integration() {
        let fetcher = RealHttpFetcher::new();
        let fake_hash =
            Hash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();

        let mut reader =
            HttpArtifactReader::new(fetcher, "https://www.google.com/".to_string(), None).unwrap();

        let result = reader.read_bytes(Path::new(&fake_hash.to_string()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        let fetch_err = err.downcast_ref::<FetchError>().unwrap();
        assert!(matches!(fetch_err, FetchError::NotFound));
    }

    #[test]
    fn test_artifact_reader_decompression_failure() {
        let mut responses = HashMap::new();
        let expected_hash =
            Hash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();

        responses.insert(test_blob_url(1, &expected_hash), Ok(vec![0, 1, 2, 3]));

        let fetcher = MockHttpFetcher::new(responses);
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to decompress"));
    }

    #[test]
    fn test_artifact_reader_invalid_hash_path() {
        let fetcher = MockHttpFetcher::new(HashMap::new());
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let result = reader.read_bytes(Path::new("not-a-merkle-hash"));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid expected Merkle hash"));
    }

    #[test]
    fn test_artifact_reader_empty_response_body() {
        let mut responses = HashMap::new();
        let expected_hash =
            Hash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();

        responses.insert(test_blob_url(1, &expected_hash), Ok(vec![]));

        let fetcher = MockHttpFetcher::new(responses);
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to decompress"));
    }

    #[test]
    fn test_artifact_reader_http_server_error() {
        let mut responses = HashMap::new();
        let expected_hash =
            Hash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();

        responses.insert(test_blob_url(1, &expected_hash), Err(FetchError::HttpError(500)));

        let fetcher = MockHttpFetcher::new(responses);
        let mut reader = HttpArtifactReader::new(fetcher, TEST_URL.to_string(), None).unwrap();
        let result = reader.read_bytes(Path::new(&expected_hash.to_string()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        let fetch_err = err.downcast_ref::<FetchError>().unwrap();
        assert!(matches!(fetch_err, FetchError::HttpError(500)));
    }
}
