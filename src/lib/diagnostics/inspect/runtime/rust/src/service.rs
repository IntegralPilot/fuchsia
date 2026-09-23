// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of the `fuchsia.inspect.Tree` protocol server.

use crate::TreeServerSendPreference;
use anyhow::Error;
use fidl_fuchsia_inspect::{
    TreeContent, TreeMarker, TreeNameIteratorRequest, TreeNameIteratorRequestStream, TreeRequest,
    TreeRequestStream,
};
use fidl_fuchsia_mem::Buffer;
use fuchsia_async as fasync;
use fuchsia_inspect::Inspector;
use fuchsia_inspect::reader::ReadableTree;
use futures::{FutureExt, TryFutureExt, TryStreamExt};
use log::warn;
use zx::sys::ZX_CHANNEL_MAX_MSG_BYTES;

/// Runs a server for the `fuchsia.inspect.Tree` protocol. This protocol returns the VMO
/// associated with the given tree on `get_content` and allows to open linked trees (lazy nodes).
#[allow(clippy::manual_async_fn)] // required because of recursion
pub fn handle_request_stream(
    inspector: Inspector,
    settings: TreeServerSendPreference,
    mut stream: TreeRequestStream,
    scope: fasync::Scope,
) -> impl futures::Future<Output = Result<(), Error>> + Send {
    async move {
        while let Some(request) = stream.try_next().await? {
            match request {
                TreeRequest::GetContent { responder } => {
                    // If freezing fails, full snapshot algo needed on live duplicate
                    let vmo = match settings {
                        TreeServerSendPreference::DeepCopy => inspector.copy_vmo(),
                        TreeServerSendPreference::Live => inspector.duplicate_vmo(),
                        TreeServerSendPreference::Frozen { ref on_failure } => {
                            match inspector.frozen_vmo_copy() {
                                Ok(vmo) => Some(vmo),
                                Err(_) => match **on_failure {
                                    TreeServerSendPreference::DeepCopy => inspector.copy_vmo(),
                                    TreeServerSendPreference::Live => inspector.duplicate_vmo(),
                                    _ => None,
                                },
                            }
                        }
                    };

                    let buffer_data =
                        vmo.and_then(|vmo| vmo.get_size().ok().map(|size| (vmo, size)));
                    let content = TreeContent {
                        buffer: buffer_data.map(|data| Buffer { vmo: data.0, size: data.1 }),
                        ..Default::default()
                    };
                    responder.send(content)?;
                }
                TreeRequest::ListChildNames { tree_iterator, .. } => {
                    let request_stream = tree_iterator.into_stream();
                    let inspector = inspector.clone();
                    scope.spawn(async move {
                        inspector
                            .tree_names()
                            .map_err(|e| anyhow::anyhow!("{e:?}"))
                            .and_then(|values| {
                                run_tree_name_iterator_server(values, request_stream)
                            })
                            .map_err(|err| {
                                warn!(err:?; "failed to run tree name iterator server");
                            })
                            .map(|_| {})
                            .await
                    });
                }
                TreeRequest::OpenChild { child_name, tree, .. } => {
                    let nested = scope.new_child_with_name("nested_tree_server");
                    let inspector = inspector.clone();
                    let settings = settings.clone();
                    let stream = tree.into_stream();
                    scope.spawn(async move {
                        inspector
                            .read_tree(&child_name)
                            .map_err(|e| anyhow::anyhow!("{e:?}"))
                            .and_then(|inspector| {
                                handle_request_stream(inspector, settings, stream, nested)
                            })
                            .map_err(|err| {
                                warn!(err:?; "failed to run `fuchsia.inspect.Tree` server");
                            })
                            .map(|_| {})
                            .await
                    });
                }
                TreeRequest::_UnknownMethod { ordinal, method_type, .. } => {
                    warn!(ordinal, method_type:?; "Unknown request");
                }
            }
        }

        scope.join().await;

        Ok(())
    }
}

/// Spawns a server for the `fuchsia.inspect.Tree` protocol. This protocol returns the VMO
/// associated with the given tree on `get_content` and allows to open linked trees (lazy nodes).
///
/// This version of the function accepts a `TreeRequestStream`, making it suitable for the
/// recursive calls performed by the `OpenChild` method on `fuchsia.inspect.Tree`.
/// `spawn_tree_server` is a more ergonomic option for spawning the root tree.
pub fn spawn_tree_server_with_stream(
    inspector: Inspector,
    settings: TreeServerSendPreference,
    stream: TreeRequestStream,
    scope: &fasync::ScopeHandle,
) {
    scope.spawn(
        handle_request_stream(
            inspector,
            settings,
            stream,
            scope.new_child_with_name("tree_server"),
        )
        .map(|e| {
            e.unwrap_or_else(
                |err: Error| warn!(err:?; "failed to run `fuchsia.inspect.Tree` server"),
            );
        }),
    );
}

/// Spawns a `fuchsia.inspect.Tree` server and returns the task handling
/// `fuchsia.inspect.Tree requests and a `ClientEnd` handle to the tree.
pub fn spawn_tree_server(
    inspector: Inspector,
    settings: TreeServerSendPreference,
    scope: &fasync::ScopeHandle,
) -> fidl::endpoints::ClientEnd<TreeMarker> {
    let (tree, server_end) = fidl::endpoints::create_endpoints::<TreeMarker>();
    spawn_tree_server_with_stream(inspector, settings, server_end.into_stream(), scope);
    tree
}

/// Runs a server for the `fuchsia.inspect.TreeNameIterator` protocol. This protocol returns the
/// given list of values by chunks.
async fn run_tree_name_iterator_server(
    values: Vec<String>,
    mut stream: TreeNameIteratorRequestStream,
) -> Result<(), anyhow::Error> {
    let mut values_iter = values.into_iter().peekable();
    while let Some(request) = stream.try_next().await? {
        match request {
            TreeNameIteratorRequest::GetNext { responder } => {
                let mut bytes_used: usize = 32; // Page overhead of message header + vector
                let mut result = vec![];
                loop {
                    match values_iter.peek() {
                        None => break,
                        Some(value) => {
                            let next_bytes = bytes_used
                                + 16 // String overhead
                                + fidl::encoding::round_up_to_align(value.len(), 8);
                            if next_bytes > ZX_CHANNEL_MAX_MSG_BYTES as usize {
                                if result.is_empty() {
                                    warn!("Dropping tree name that exceeds channel message size");
                                    values_iter.next();
                                    continue;
                                }
                                break;
                            }
                            bytes_used = next_bytes;
                            result.push(values_iter.next().unwrap());
                        }
                    }
                }
                if result.is_empty() {
                    responder.send(&[])?;
                    return Ok(());
                }
                responder.send(&result)?;
            }
            TreeNameIteratorRequest::_UnknownMethod { ordinal, method_type, .. } => {
                warn!(ordinal, method_type:?; "Unknown request");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{assert_data_tree, assert_json_diff};
    use fidl_fuchsia_inspect::{TreeNameIteratorMarker, TreeNameIteratorProxy, TreeProxy};
    use fuchsia_async::DurationExt;
    use fuchsia_inspect::reader::{DiagnosticsHierarchy, PartialNodeHierarchy, read_with_timeout};
    use std::sync::Arc;

    use futures::FutureExt;
    use std::time::Duration;

    /// Spawns a `fuchsia.inspect.Tree` server and returns the task handling
    /// `fuchsia.inspect.Tree` requests and a `TreeProxy` handle to the tree.
    pub fn spawn_server_proxy(
        inspector: Inspector,
        settings: TreeServerSendPreference,
    ) -> (Arc<fasync::Scope>, TreeProxy) {
        let scope = Arc::new(fasync::Scope::new());
        (scope.clone(), spawn_tree_server(inspector, settings, scope.as_handle()).into_proxy())
    }

    #[fuchsia::test]
    async fn get_contents() -> Result<(), Error> {
        let (_server, tree) =
            spawn_server_proxy(test_inspector(), TreeServerSendPreference::default());
        let tree_content = tree.get_content().await?;
        let hierarchy = parse_content(tree_content)?;
        assert_data_tree!(hierarchy, root: {
            a: 1i64,
        });
        Ok(())
    }

    #[fuchsia::test]
    async fn list_child_names() -> Result<(), Error> {
        let (_server, tree) =
            spawn_server_proxy(test_inspector(), TreeServerSendPreference::default());
        let (name_iterator, server_end) = fidl::endpoints::create_proxy::<TreeNameIteratorMarker>();
        tree.list_child_names(server_end)?;
        verify_iterator(name_iterator, vec!["lazy-0".to_string()]).await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn oversized_tree_name_is_dropped() -> Result<(), Error> {
        let (name_iterator, server_end) = fidl::endpoints::create_proxy::<TreeNameIteratorMarker>();
        let scope = fasync::Scope::new();
        scope.spawn(async move {
            let oversized = "a".repeat(ZX_CHANNEL_MAX_MSG_BYTES as usize);
            run_tree_name_iterator_server(
                vec![oversized, "normal".to_string()],
                server_end.into_stream(),
            )
            .await
            .unwrap();
        });
        verify_iterator(name_iterator, vec!["normal".to_string()]).await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn open_children() -> Result<(), Error> {
        let (_server, tree) =
            spawn_server_proxy(test_inspector(), TreeServerSendPreference::default());
        let (child_tree, server_end) = fidl::endpoints::create_proxy::<TreeMarker>();
        tree.open_child("lazy-0", server_end)?;
        let tree_content = child_tree.get_content().await?;
        let hierarchy = parse_content(tree_content)?;
        assert_data_tree!(hierarchy, root: {
            b: 2u64,
        });
        let (name_iterator, server_end) = fidl::endpoints::create_proxy::<TreeNameIteratorMarker>();
        child_tree.list_child_names(server_end)?;
        verify_iterator(name_iterator, vec!["lazy-vals-0".to_string()]).await?;

        let (child_tree_2, server_end) = fidl::endpoints::create_proxy::<TreeMarker>();
        child_tree.open_child("lazy-vals-0", server_end)?;
        let tree_content = child_tree_2.get_content().await?;
        let hierarchy = parse_content(tree_content)?;
        assert_data_tree!(hierarchy, root: {
            c: 3.0,
        });
        let (name_iterator, server_end) = fidl::endpoints::create_proxy::<TreeNameIteratorMarker>();
        child_tree_2.list_child_names(server_end)?;
        verify_iterator(name_iterator, vec![]).await?;

        Ok(())
    }

    #[fuchsia::test]
    async fn default_snapshots_are_private_on_success() -> Result<(), Error> {
        let inspector = test_inspector();
        let (_server, tree_copy) =
            spawn_server_proxy(inspector.clone(), TreeServerSendPreference::default());
        let tree_content_copy = tree_copy.get_content().await?;

        inspector.root().record_int("new", 6);

        // A tree that copies the vmo doesn't see the new int
        let hierarchy = parse_content(tree_content_copy)?;
        assert_data_tree!(hierarchy, root: {
            a: 1i64,
        });
        Ok(())
    }

    #[fuchsia::test]
    async fn force_live_snapshot() -> Result<(), Error> {
        let inspector = test_inspector();
        let (_server1, tree_cow) =
            spawn_server_proxy(inspector.clone(), TreeServerSendPreference::default());
        let (_server2, tree_live) =
            spawn_server_proxy(inspector.clone(), TreeServerSendPreference::Live);
        let tree_content_live = tree_live.get_content().await?;
        let tree_content_cow = tree_cow.get_content().await?;

        inspector.root().record_int("new", 6);

        // A tree that cow's the vmo doesn't see the new int
        let hierarchy = parse_content(tree_content_cow)?;
        assert_data_tree!(hierarchy, root: {
            a: 1i64,
        });

        // A tree that live-duplicates the vmo sees the new int
        let hierarchy = parse_content(tree_content_live)?;
        assert_data_tree!(hierarchy, root: {
            a: 1i64,
            new: 6i64,
        });
        Ok(())
    }

    #[fuchsia::test]
    async fn read_hanging_lazy_node() -> Result<(), Error> {
        let inspector = Inspector::default();
        let root = inspector.root();
        root.record_string("child", "value");

        root.record_lazy_values("lazy-node-always-hangs", || {
            async move {
                fuchsia_async::Timer::new(zx::MonotonicDuration::from_minutes(30).after_now())
                    .await;
                Ok(Inspector::default())
            }
            .boxed()
        });

        root.record_lazy_values("lazy-node-doesnt-hang", || {
            async move {
                let inspector = Inspector::default();
                inspector.root().record_string("from", "non-hanging");
                Ok(inspector)
            }
            .boxed()
        });

        root.record_int("int", 3);

        let instrumentation = Inspector::default();
        let counter = instrumentation.root().create_uint("counter", 0);

        let (_server, proxy) = spawn_server_proxy(inspector, TreeServerSendPreference::default());
        let result = read_with_timeout(&proxy, Duration::from_secs(5), &counter).await?;

        assert_json_diff!(instrumentation, root: { counter: 1 });

        assert_json_diff!(result, root: {
            child: "value",
            int: 3i64,
            from: "non-hanging",
        });

        Ok(())
    }

    async fn verify_iterator(
        name_iterator: TreeNameIteratorProxy,
        values: Vec<String>,
    ) -> Result<(), Error> {
        if !values.is_empty() {
            assert_eq!(name_iterator.get_next().await?, values);
        }
        assert!(name_iterator.get_next().await?.is_empty());
        assert!(name_iterator.get_next().await.is_err());
        Ok(())
    }

    fn parse_content(tree_content: TreeContent) -> Result<DiagnosticsHierarchy, Error> {
        let buffer = tree_content.buffer.unwrap();
        Ok(PartialNodeHierarchy::try_from(&buffer.vmo)?.into())
    }

    fn test_inspector() -> Inspector {
        let inspector = Inspector::default();
        inspector.root().record_int("a", 1);
        inspector.root().record_lazy_child("lazy", || {
            async move {
                let inspector = Inspector::default();
                inspector.root().record_uint("b", 2);
                inspector.root().record_lazy_values("lazy-vals", || {
                    async move {
                        let inspector = Inspector::default();
                        inspector.root().record_double("c", 3.0);
                        Ok(inspector)
                    }
                    .boxed()
                });
                Ok(inspector)
            }
            .boxed()
        });
        inspector
    }
}
