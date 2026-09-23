// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cache::BlobFetchParams;
use fidl_fuchsia_pkg_ext::BlobId;
use fuchsia_inspect::{IntProperty, Node, Property as _, StringProperty};

use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

fn now_monotonic_nanos() -> i64 {
    zx::MonotonicInstant::get().into_nanos()
}

/// Creates Inspect wrappers for individual blob fetches.
pub struct BlobFetcher {
    queue: Node,
    _node: Node,
}

impl BlobFetcher {
    /// Create a `BlobFetcher` from an Inspect node.
    pub fn from_node_and_params(node: Node, params: &BlobFetchParams) -> Self {
        node.record_int(
            "blob_header_timeout_seconds",
            params.header_network_timeout().into_seconds(),
        );
        node.record_int("blob_body_timeout_seconds", params.body_network_timeout().into_seconds());
        node.record_uint(
            "blob_download_resumption_attempts_limit",
            params.download_resumption_attempts_limit().into(),
        );
        Self { queue: node.create_child("queue"), _node: node }
    }

    /// Create an Inspect wrapper for an individual blob fetch.
    pub fn fetch(&self, id: &BlobId) -> NeedsRemoteType {
        let node = self.queue.create_child(id.to_string());
        node.record_int("fetch_ts", now_monotonic_nanos());
        NeedsRemoteType { node }
    }
}

/// A blob fetch that the pkg-resolver has begun processing.
pub struct NeedsRemoteType {
    node: Node,
}

impl NeedsRemoteType {
    /// Mark that the blob contents will be obtained via http.
    pub fn http(self) -> TriggerAttempt<Http> {
        self.node.record_string("source", "http");
        TriggerAttempt::<Http>::new(self.node)
    }
}

pub(crate) struct TriggerAttempt<S: State> {
    attempt_count: AtomicU32,
    attempts: Node,
    node: Node,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: State> TriggerAttempt<S> {
    fn new(node: Node) -> Self {
        Self {
            attempt_count: AtomicU32::new(0),
            attempts: node.create_child("attempts"),
            node,
            _phantom: PhantomData,
        }
    }

    pub fn set_mirror(&self, mirror: &str) {
        self.node.record_string("mirror", mirror);
    }

    pub fn attempt(&self) -> Attempt<S> {
        // Don't zero-index attempts so it is obvious in inspect that multiple attempts
        // have occurred.
        let index = 1 + self.attempt_count.fetch_add(1, Ordering::SeqCst);
        let node = self.attempts.create_child(index.to_string());
        let state = node.create_string("state", "initial");
        let state_ts = node.create_int("state_ts", now_monotonic_nanos());
        Attempt::<S> { state, state_ts, _node: node, _phantom: PhantomData }
    }
}

/// Sub-states for an http fetch.
pub enum Http {
    CreateBlob,
    DownloadBlob,
    CloseBlob,
}

/// A sub-state for a fetch. The stringification will be exported via Inspect.
pub trait State {
    fn as_str(&self) -> &'static str;
}

impl State for Http {
    fn as_str(&self) -> &'static str {
        match self {
            Http::CreateBlob => "create blob",
            Http::DownloadBlob => "download blob",
            Http::CloseBlob => "close blob",
        }
    }
}

/// The terminal type of the fetch Inspect wrappers. This ends the use of move semantics to enforce
/// type transitions because at this point in cache.rs the type is being passed into and out of
/// functions and captured by FnMut.
pub struct Attempt<S: State> {
    state: StringProperty,
    state_ts: IntProperty,
    _node: Node,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: State> Attempt<S> {
    /// Change the sub-state of this fetch.
    pub fn state(&self, state: S) {
        self.state.set(state.as_str());
        self.state_ts.set(now_monotonic_nanos());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;

    const ZEROES_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const ONES_HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    impl BlobFetcher {
        fn from_node(node: Node) -> Self {
            Self::from_node_and_params(
                node,
                &BlobFetchParams::builder()
                    .header_network_timeout(zx::BootDuration::from_seconds(0))
                    .body_network_timeout(zx::BootDuration::from_seconds(1))
                    .download_resumption_attempts_limit(2)
                    .build(),
            )
        }
    }

    #[fuchsia::test]
    async fn initial_state() {
        let inspector = Inspector::default();

        let _blob_fetcher = BlobFetcher::from_node(inspector.root().create_child("blob_fetcher"));
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: {
                    blob_header_timeout_seconds: 0i64,
                    blob_body_timeout_seconds: 1i64,
                    blob_download_resumption_attempts_limit: 2u64,
                    queue: {}
                }
            }
        );
    }

    #[fuchsia::test]
    async fn http_state_progression() {
        let inspector = Inspector::default();

        let blob_fetcher = BlobFetcher::from_node(inspector.root().create_child("blob_fetcher"));
        let inspect = blob_fetcher.fetch(&BlobId::parse(ZEROES_HASH).unwrap());
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => {
                            fetch_ts: AnyProperty,
                        }
                    }
                }
            }
        );

        let inspect = inspect.http();
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => {
                            fetch_ts: AnyProperty,
                            source: "http",
                            attempts: {},
                        }
                    }
                }
            }
        );

        inspect.set_mirror("fake-mirror");
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => {
                            fetch_ts: AnyProperty,
                            source: "http",
                            mirror: "fake-mirror",
                            attempts: {},
                        }
                    }
                }
            }
        );

        let attempt = inspect.attempt();
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => {
                            fetch_ts: AnyProperty,
                            source: "http",
                            mirror: "fake-mirror",
                            attempts: {
                                "1": {
                                    state: "initial",
                                    state_ts: AnyProperty,
                                }
                            },
                        }
                    }
                }
            }
        );

        attempt.state(Http::CreateBlob);
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => {
                            fetch_ts: AnyProperty,
                            source: "http",
                            mirror: "fake-mirror",
                            attempts: {
                                "1": {
                                    state: "create blob",
                                    state_ts: AnyProperty,
                                }
                            },
                        }
                    }
                }
            }
        );

        drop(attempt);
        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => {
                            fetch_ts: AnyProperty,
                            source: "http",
                            mirror: "fake-mirror",
                            attempts: {},
                        }
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    async fn multiple_fetches() {
        let inspector = Inspector::default();

        let blob_fetcher = BlobFetcher::from_node(inspector.root().create_child("blob_fetcher"));
        let _inspect0 = blob_fetcher.fetch(&BlobId::parse(ZEROES_HASH).unwrap());
        let _inspect1 = blob_fetcher.fetch(&BlobId::parse(ONES_HASH).unwrap());

        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => contains {},
                        ONES_HASH.to_string() => contains {},
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    async fn multiple_attempts() {
        let inspector = Inspector::default();

        let blob_fetcher = BlobFetcher::from_node(inspector.root().create_child("blob_fetcher"));
        let inspect = blob_fetcher.fetch(&BlobId::parse(ZEROES_HASH).unwrap()).http();
        let _attempt0 = inspect.attempt();
        let _attempt1 = inspect.attempt();

        assert_data_tree!(
            inspector,
            root: {
                blob_fetcher: contains {
                    queue: {
                        ZEROES_HASH.to_string() => contains {
                            attempts: {
                                "1": contains {},
                                "2": contains {},
                            }
                        }
                    }
                }
            }
        );
    }
}
