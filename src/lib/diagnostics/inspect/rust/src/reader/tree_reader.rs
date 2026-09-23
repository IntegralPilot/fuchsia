// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::reader::error::ReaderError;
use crate::reader::{
    DiagnosticsHierarchy, MissingValueReason, PartialNodeHierarchy, ReadableTree, Snapshot,
};
use crate::{NumericProperty, UintProperty};
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_sync::Mutex;
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use inspect_format::LinkNodeDisposition;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// Ten minutes, an absurdly long time to try and read Inspect
///
/// Note: do not replace with Duration::MAX. It will overflow in the
/// implementation of `DurationExt::after_now` used for timeouts.
const MAX_READ_TIME: std::time::Duration = std::time::Duration::from_secs(60 * 10);

/// Contains the snapshot of the hierarchy and snapshots of all the lazy nodes in the hierarchy.
#[derive(Debug)]
pub struct SnapshotTree {
    snapshot: Snapshot,
    children: SnapshotTreeMap,
}

impl SnapshotTree {
    /// Loads a snapshot tree from the given inspect tree.
    #[cfg(target_os = "fuchsia")]
    pub async fn try_from_proxy(
        tree: &fidl_fuchsia_inspect::TreeProxy,
    ) -> Result<SnapshotTree, ReaderError> {
        load_snapshot_tree(tree, MAX_READ_TIME, &Default::default()).await
    }

    pub async fn try_from_with_timeout<T: ReadableTree + Send + Sync + Clone>(
        tree: &T,
        lazy_child_timeout: Duration,
        timeout_counter: &UintProperty,
    ) -> Result<SnapshotTree, ReaderError> {
        load_snapshot_tree(tree, lazy_child_timeout, timeout_counter).await
    }
}

impl<'a, T> TryFrom<Trees<'a, T>> for SnapshotTree
where
    T: ReadableTree + Send + Sync + Clone,
{
    type Error = ReaderError;

    fn try_from(trees: Trees<'a, T>) -> Result<Self, Self::Error> {
        let snapshot =
            trees.resolved_node.into_inner().expect("lazy initialization must be performed")?;

        let mut children_map: SnapshotTreeMap = BTreeMap::new();

        // Iterate through successfully discovered children Arc<Trees<T>>.
        for child_arc in trees.children.into_inner().into_iter() {
            // Get the child's name to use as the key in the map.
            if child_arc.name.is_some() {
                let Some(mut child) = Arc::into_inner(child_arc) else {
                    return Err(ReaderError::Internal);
                };
                let name = child.name.take().unwrap_or_default();
                let child_result = SnapshotTree::try_from(child);

                children_map.insert(name, child_result);
            }
            // If a child has no name, it cannot be represented in the map; skip it.
        }

        for (name, error) in trees.child_errors.into_inner().into_iter() {
            children_map.insert(name, Err(error));
        }

        Ok(SnapshotTree { snapshot, children: children_map })
    }
}

type SnapshotTreeMap = BTreeMap<String, Result<SnapshotTree, ReaderError>>;

impl TryInto<DiagnosticsHierarchy> for SnapshotTree {
    type Error = ReaderError;

    fn try_into(mut self) -> Result<DiagnosticsHierarchy, Self::Error> {
        let partial = PartialNodeHierarchy::try_from(self.snapshot)?;
        Ok(expand(partial, &mut self.children))
    }
}

const MAX_EXPAND_DEPTH: usize = 128;

fn expand(
    partial: PartialNodeHierarchy,
    snapshot_children: &mut SnapshotTreeMap,
) -> DiagnosticsHierarchy {
    expand_inner(partial, snapshot_children, 0)
}

fn expand_inner(
    partial: PartialNodeHierarchy,
    snapshot_children: &mut SnapshotTreeMap,
    depth: usize,
) -> DiagnosticsHierarchy {
    let children = if depth >= MAX_EXPAND_DEPTH {
        vec![]
    } else {
        partial
            .children
            .into_iter()
            .map(|child| expand_inner(child, snapshot_children, depth + 1))
            .collect()
    };
    let mut hierarchy = DiagnosticsHierarchy::new(partial.name, partial.properties, children);
    for link_value in partial.links {
        let Some(result) = snapshot_children.remove(&link_value.content) else {
            hierarchy.add_missing(MissingValueReason::LinkNotFound, link_value.name);
            continue;
        };

        if depth >= MAX_EXPAND_DEPTH {
            hierarchy.add_missing(MissingValueReason::MaxDepthExceeded, link_value.name);
            continue;
        }

        let result: Result<DiagnosticsHierarchy, ReaderError> =
            result.and_then(|snapshot_tree| snapshot_tree.try_into());
        match result {
            Err(ReaderError::TreeTimedOut) => {
                hierarchy.add_missing(MissingValueReason::Timeout, link_value.name);
            }
            Err(_) => {
                hierarchy.add_missing(MissingValueReason::LinkParseFailure, link_value.name);
            }
            Ok(mut child_hierarchy) => match link_value.disposition {
                LinkNodeDisposition::Child => {
                    child_hierarchy.name = link_value.name;
                    hierarchy.children.push(child_hierarchy);
                }
                LinkNodeDisposition::Inline => {
                    hierarchy.children.extend(child_hierarchy.children);
                    hierarchy.properties.extend(child_hierarchy.properties);
                    hierarchy.missing.extend(child_hierarchy.missing);
                }
            },
        }
    }
    hierarchy
}

/// Reads the given `ReadableTree` into a DiagnosticsHierarchy.
/// This reads versions 1 and 2 of the Inspect Format.
pub async fn read<T>(tree: &T) -> Result<DiagnosticsHierarchy, ReaderError>
where
    T: ReadableTree + Send + Sync + Clone,
{
    load_snapshot_tree(tree, MAX_READ_TIME, &Default::default()).await?.try_into()
}

/// Reads the given `ReadableTree` into a DiagnosticsHierarchy with Lazy Node timeout.
/// This reads versions 1 and 2 of the Inspect Format.
pub async fn read_with_timeout<T>(
    tree: &T,
    lazy_node_timeout: Duration,
    timeout_counter: &UintProperty,
) -> Result<DiagnosticsHierarchy, ReaderError>
where
    T: ReadableTree + Send + Sync + Clone,
{
    load_snapshot_tree(tree, lazy_node_timeout, timeout_counter).await?.try_into()
}

/// Intermediate type for representing the tree structure of a proxy/Inspector while
/// inflating nodes in an online-manner.
struct Trees<'a, T: Clone> {
    node: Cow<'a, T>,
    name: Option<String>,
    resolved_node: Mutex<Option<Result<Snapshot, ReaderError>>>,
    // TODO: b/431224266 - Explore using bumpalo rather than Arc for these allocations
    children: Mutex<Vec<Arc<Trees<'a, T>>>>,
    parent: Mutex<Option<Weak<Trees<'a, T>>>>,
    child_errors: Mutex<BTreeMap<String, ReaderError>>,
}

async fn load_snapshot_tree<'a, T>(
    tree: &'a T,
    timeout: Duration,
    timeout_counter: &UintProperty,
) -> Result<SnapshotTree, ReaderError>
where
    T: ReadableTree + Send + Sync + Clone,
{
    let trees = Arc::new(Trees {
        node: Cow::Borrowed(tree),
        name: None,
        children: Mutex::new(vec![]),
        parent: Mutex::new(None),
        resolved_node: Mutex::new(None),
        child_errors: Mutex::new(BTreeMap::new()),
    });
    let vmo_resolver = FuturesUnordered::new();
    let moveable_for_read_tree_queue = Arc::clone(&trees);
    let mut read_tree_resolver = FuturesUnordered::<
        Pin<Box<dyn Future<Output = Option<Arc<Trees<'a, T>>>> + Send>>,
    >::from_iter([
        async move { Some(moveable_for_read_tree_queue) }.boxed(),
    ]);

    while let Some(current) = read_tree_resolver.next().await {
        let Some(current) = current else {
            continue;
        };
        let moveable = Arc::clone(&current);
        vmo_resolver.push(async move {
            let resolution = moveable
                .node
                .vmo()
                .on_timeout(timeout.after_now(), || {
                    timeout_counter.add(1);
                    Err(ReaderError::TreeTimedOut)
                })
                .await
                .and_then(|data| Snapshot::try_from(&data));
            let _ = moveable.resolved_node.lock().insert(resolution);
        });
        let tree_names = match current
            .node
            .tree_names()
            .on_timeout(timeout.after_now(), || {
                // no timeout_counter increment, because a failure here
                // will also manifest as failing to read the VMO of this node
                Err(ReaderError::TreeTimedOut)
            })
            .await
        {
            Ok(tree_names) => tree_names,
            Err(err) => {
                let guard = current.parent.lock();
                let Some(Some(parent)) = guard.as_ref().map(|weak| weak.upgrade()) else {
                    // If we can't get the parent, then we can't register an error. Oh well.
                    // We also can't read any children, since the tree_names call didn't work.
                    // So, just move to the next node and give up.
                    continue;
                };
                if let Some(name) = &current.name {
                    parent.child_errors.lock().insert(String::clone(name), err);
                }
                continue;
            }
        };

        for child in tree_names.into_iter() {
            let current = Arc::clone(&current);
            let next_node = async move {
                current
                    .node
                    .read_tree(&child)
                    .map(|node| match node {
                        Ok(node) => {
                            let next = Arc::new(Trees {
                                node: Cow::Owned(node),
                                name: Some(String::clone(&child)),
                                resolved_node: Mutex::new(None),
                                children: Mutex::new(vec![]),
                                parent: Mutex::new(Some(Arc::downgrade(&current))),
                                child_errors: Mutex::new(BTreeMap::new()),
                            });
                            let mut guard = current.children.lock();
                            guard.push(next);
                            guard.last().map(Arc::clone)
                        }
                        Err(e) => {
                            current.child_errors.lock().insert(String::clone(&child), e);
                            None
                        }
                    })
                    .on_timeout(timeout.after_now(), || {
                        timeout_counter.add(1);
                        current
                            .child_errors
                            .lock()
                            .insert(String::clone(&child), ReaderError::TreeTimedOut);
                        None
                    })
                    .await
            };
            read_tree_resolver.push(next_node.boxed());
        }
    }

    let _ = vmo_resolver.collect::<Vec<()>>().await;
    Arc::into_inner(trees).map(SnapshotTree::try_from).unwrap_or(Err(ReaderError::Internal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Inspector, InspectorConfig, reader};
    use diagnostics_assertions::{assert_data_tree, assert_json_diff};
    use inspect_format::constants;

    #[fuchsia::test]
    async fn test_read() -> Result<(), anyhow::Error> {
        let inspector = test_inspector();
        let hierarchy = read(&inspector).await?;
        assert_data_tree!(hierarchy, root: {
            int: 3i64,
            "lazy-node": {
                a: "test",
                child: {
                    double: 3.25,
                },
            }
        });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_load_snapshot_tree() -> Result<(), anyhow::Error> {
        let instrumentation = Inspector::default();
        let counter = instrumentation.root().create_uint("counter", 0);

        let inspector = test_inspector();
        let mut snapshot_tree = load_snapshot_tree(&inspector, MAX_READ_TIME, &counter).await?;

        assert_data_tree!(instrumentation, root: { counter: 0u64 });

        let root_hierarchy: DiagnosticsHierarchy =
            PartialNodeHierarchy::try_from(snapshot_tree.snapshot)?.into();
        assert_eq!(snapshot_tree.children.keys().collect::<Vec<&String>>(), vec!["lazy-node-0"]);
        assert_data_tree!(root_hierarchy, root: {
            int: 3i64,
        });

        let mut lazy_node = snapshot_tree.children.remove("lazy-node-0").unwrap().unwrap();
        let lazy_node_hierarchy: DiagnosticsHierarchy =
            PartialNodeHierarchy::try_from(lazy_node.snapshot)?.into();
        assert_eq!(lazy_node.children.keys().collect::<Vec<&String>>(), vec!["lazy-values-0"]);
        assert_data_tree!(lazy_node_hierarchy, root: {
            a: "test",
            child: {},
        });

        let lazy_values = lazy_node.children.remove("lazy-values-0").unwrap().unwrap();
        let lazy_values_hierarchy = PartialNodeHierarchy::try_from(lazy_values.snapshot)?;
        assert_eq!(lazy_values.children.keys().len(), 0);
        assert_data_tree!(lazy_values_hierarchy, root: {
            double: 3.25,
        });

        Ok(())
    }

    #[fuchsia::test]
    async fn read_with_hanging_lazy_node() -> Result<(), anyhow::Error> {
        let instrumentation = Inspector::default();
        let counter = instrumentation.root().create_uint("counter", 0);

        let inspector = Inspector::default();
        let root = inspector.root();
        root.record_string("child", "value");

        root.record_lazy_values("lazy-node-always-hangs", || {
            async move {
                fuchsia_async::Timer::new(Duration::from_secs(30 * 60).after_now()).await;
                Ok(Inspector::default())
            }
            .boxed()
        });

        root.record_int("int", 3);

        let hierarchy = read_with_timeout(&inspector, Duration::from_secs(2), &counter).await?;

        assert_data_tree!(instrumentation, root: { counter: 1u64 });

        assert_json_diff!(hierarchy, root: {
            child: "value",
            int: 3i64,
        });

        Ok(())
    }

    #[fuchsia::test]
    async fn read_too_big_string() {
        // magic size is the amount of data that can be written. Basically it is the size of the
        // VMO minus all of the header and bookkeeping data stored in the VMO
        let magic_size_found_by_experiment = 259076;
        let inspector =
            Inspector::new(InspectorConfig::default().size(constants::DEFAULT_VMO_SIZE_BYTES));
        let string_head = "X".repeat(magic_size_found_by_experiment);
        let string_tail =
            "Y".repeat((constants::DEFAULT_VMO_SIZE_BYTES * 2) - magic_size_found_by_experiment);
        let full_string = format!("{string_head}{string_tail}");

        inspector.root().record_int(full_string, 5);
        let hierarchy = reader::read(&inspector).await.unwrap();
        // somewhat redundant to check both things, but checking the size makes fencepost errors
        // obvious, while checking the contents make real problems obvious
        assert_eq!(hierarchy.properties[0].key().len(), string_head.len());
        assert_eq!(hierarchy.properties[0].key(), &string_head);
    }

    #[fuchsia::test]
    async fn missing_value_parse_failure() -> Result<(), anyhow::Error> {
        let inspector = Inspector::default();
        let _lazy_child = inspector.root().create_lazy_child("lazy", || {
            async move {
                // For the sake of the test, force an invalid vmo.
                Ok(Inspector::new(InspectorConfig::default().no_op()))
            }
            .boxed()
        });
        let hierarchy = reader::read(&inspector).await?;
        assert_eq!(hierarchy.missing.len(), 1);
        assert_eq!(hierarchy.missing[0].reason, MissingValueReason::LinkParseFailure);
        assert_data_tree!(hierarchy, root: {});
        Ok(())
    }

    #[fuchsia::test]
    async fn missing_value_not_found() -> Result<(), anyhow::Error> {
        let inspector = Inspector::default();
        if let Some(state) = inspector.state() {
            let mut state = state.try_lock().expect("lock state");
            state
                .allocate_link("missing", "missing-404", LinkNodeDisposition::Child, 0.into())
                .unwrap();
        }
        let hierarchy = reader::read(&inspector).await?;
        assert_eq!(hierarchy.missing.len(), 1);
        assert_eq!(hierarchy.missing[0].reason, MissingValueReason::LinkNotFound);
        assert_eq!(hierarchy.missing[0].name, "missing");
        assert_data_tree!(hierarchy, root: {});
        Ok(())
    }

    #[fuchsia::test]
    async fn missing_value_max_depth_exceeded() {
        let partial = PartialNodeHierarchy {
            name: "root".to_string(),
            properties: vec![],
            children: vec![],
            links: vec![crate::reader::LinkValue {
                name: "link".to_string(),
                content: "link-content".to_string(),
                disposition: LinkNodeDisposition::Child,
            }],
        };
        let mut snapshot_children = SnapshotTreeMap::new();
        snapshot_children.insert(
            "link-content".to_string(),
            Ok(SnapshotTree {
                snapshot: Snapshot::try_from(&Inspector::default().vmo().await.unwrap()).unwrap(),
                children: BTreeMap::new(),
            }),
        );
        let hierarchy = expand_inner(partial, &mut snapshot_children, MAX_EXPAND_DEPTH);
        assert_eq!(hierarchy.missing.len(), 1);
        assert_eq!(hierarchy.missing[0].reason, MissingValueReason::MaxDepthExceeded);
        assert_eq!(hierarchy.missing[0].name, "link");
    }

    fn test_inspector() -> Inspector {
        let inspector = Inspector::default();
        let root = inspector.root();
        root.record_int("int", 3);
        root.record_lazy_child("lazy-node", || {
            async move {
                let inspector = Inspector::default();
                inspector.root().record_string("a", "test");
                let child = inspector.root().create_child("child");
                child.record_lazy_values("lazy-values", || {
                    async move {
                        let inspector = Inspector::default();
                        inspector.root().record_double("double", 3.25);
                        Ok(inspector)
                    }
                    .boxed()
                });
                inspector.root().record(child);
                Ok(inspector)
            }
            .boxed()
        });
        inspector
    }

    #[fuchsia::test]
    async fn test_does_not_deadlock() {
        let inspector = Inspector::default();
        let parent = inspector.root();
        let parent_clone = parent.clone_weak();
        parent.record_lazy_child("test", move || {
            let child = Inspector::default();
            parent_clone.record_int("testing", 0);
            child.root().record_int("test_2", 0);
            futures::future::ok(child).boxed()
        });
        assert_data_tree!(inspector, root: {
            test:{
                test_2: 0i64,
            },
            testing:0i64,
        });
    }

    #[fuchsia::test]
    async fn try_from_trees_for_snapshot_tree() {
        // 1. Set up the data
        let inspector_root = Inspector::default();
        inspector_root.root().record_int("val", 1);
        let vmo_root = inspector_root.vmo().await.unwrap();
        let snapshot_root = Snapshot::try_from(&vmo_root).unwrap();

        let inspector_child1 = Inspector::default();
        inspector_child1.root().record_int("val", 2);
        let vmo_child1 = inspector_child1.vmo().await.unwrap();
        let snapshot_child1 = Snapshot::try_from(&vmo_child1).unwrap();

        let child1 = Arc::new(Trees::<Inspector> {
            node: Cow::Owned(inspector_child1),
            name: Some("child1".to_string()),
            resolved_node: Mutex::new(Some(Ok(snapshot_child1))),
            children: Mutex::new(vec![]),
            parent: Mutex::new(None),
            child_errors: Mutex::new(BTreeMap::new()),
        });

        let mut child_errors = BTreeMap::new();
        child_errors.insert("child2".to_string(), ReaderError::TreeTimedOut);

        let root_trees = Trees::<Inspector> {
            node: Cow::Owned(inspector_root),
            name: Some("root".to_string()),
            resolved_node: Mutex::new(Some(Ok(snapshot_root))),
            children: Mutex::new(vec![child1]),
            parent: Mutex::new(None),
            child_errors: Mutex::new(child_errors),
        };

        // 2. Call the function to be tested
        let mut snapshot_tree = SnapshotTree::try_from(root_trees).unwrap();

        // 3. Assert the results
        let root_hierarchy: DiagnosticsHierarchy =
            PartialNodeHierarchy::try_from(snapshot_tree.snapshot).unwrap().into();
        assert_data_tree!(root_hierarchy, root: {
            val: 1i64,
        });

        assert_eq!(snapshot_tree.children.len(), 2);

        // Check the successful child
        let child1_tree = snapshot_tree.children.remove("child1").unwrap().unwrap();
        let child1_hierarchy: DiagnosticsHierarchy =
            PartialNodeHierarchy::try_from(child1_tree.snapshot).unwrap().into();
        assert_data_tree!(child1_hierarchy, root: {
            val: 2i64,
        });
        assert!(child1_tree.children.is_empty());

        // Check the error child
        let child2_error = snapshot_tree.children.remove("child2").unwrap().unwrap_err();
        assert!(matches!(child2_error, ReaderError::TreeTimedOut));
    }

    #[fuchsia::test]
    async fn try_from_trees_for_snapshot_tree_child_with_no_name() {
        // 1. Set up the data
        let inspector_root = Inspector::default();
        inspector_root.root().record_int("val", 1);
        let vmo_root = inspector_root.vmo().await.unwrap();
        let snapshot_root = Snapshot::try_from(&vmo_root).unwrap();

        let inspector_child1 = Inspector::default();
        inspector_child1.root().record_int("val", 2);
        let vmo_child1 = inspector_child1.vmo().await.unwrap();
        let snapshot_child1 = Snapshot::try_from(&vmo_child1).unwrap();

        let child1 = Arc::new(Trees::<Inspector> {
            node: Cow::Owned(inspector_child1),
            name: None, // No name
            resolved_node: Mutex::new(Some(Ok(snapshot_child1))),
            children: Mutex::new(vec![]),
            parent: Mutex::new(None),
            child_errors: Mutex::new(BTreeMap::new()),
        });

        let root_trees = Trees::<Inspector> {
            node: Cow::Owned(inspector_root),
            name: Some("root".to_string()),
            resolved_node: Mutex::new(Some(Ok(snapshot_root))),
            children: Mutex::new(vec![child1]),
            parent: Mutex::new(None),
            child_errors: Mutex::new(BTreeMap::new()),
        };

        // 2. Call the function to be tested
        let snapshot_tree = SnapshotTree::try_from(root_trees).unwrap();

        // 3. Assert the results
        // The child with no name should be skipped.
        assert!(snapshot_tree.children.is_empty());
    }
}
