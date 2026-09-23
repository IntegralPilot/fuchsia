// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::Property;
use fuchsia_sync::Mutex;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Error, Formatter};
use std::ops::Deref;
use std::sync::Arc;

/// Container to store diagnostic content in inspect.
pub struct RootDiagnosticNode {
    inspect: Arc<fuchsia_inspect::Node>,
    count: std::sync::atomic::AtomicU64,
}

impl RootDiagnosticNode {
    /// Create a new |RootDiagnosticNode| rooted at |inspect|.
    pub fn new(inspect: fuchsia_inspect::Node) -> Self {
        Self { inspect: Arc::new(inspect), count: std::sync::atomic::AtomicU64::new(0) }
    }

    /// Create a |DiagnosticNode| that records contents to inspect when dropped.
    pub(crate) fn persistent_child(&self) -> DiagnosticNode {
        let name =
            format!("id-{:?}", self.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        DiagnosticNode::new_set_persistence(name, self.inspect.clone(), true)
    }

    /// Create a |DiagnosticNode| that erases contents from inspect when dropped.
    pub(crate) fn child(&self) -> DiagnosticNode {
        let name =
            format!("id-{:?}", self.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        DiagnosticNode::new(name, self.inspect.clone())
    }
}

/// A hierarchical container for diagnostic context.
///
/// |DiagnosticNode| contains a name, any properties saved to it, and a reference to
/// it's parent, if any.
/// When printed with Debug, prints all context for ancestor roots.
/// The hierarchy of |DiagnosticNodes| is also output to inspect.
pub(crate) struct DiagnosticNode {
    inner: Arc<DiagnosticNodeInner>,
}

#[derive(Clone)]
enum Parent {
    Root(Arc<fuchsia_inspect::Node>),
    Node(Arc<DiagnosticNodeInner>),
}

struct DiagnosticNodeInner {
    name: Cow<'static, str>,
    properties: Mutex<HashMap<&'static str, (Cow<'static, str>, fuchsia_inspect::StringProperty)>>,
    parent: Parent,
    inspect: fuchsia_inspect::Node,
    persistent: bool,
}

impl DiagnosticNode {
    /// Create a new root |DiagnosticNode| using the given inspect node.
    pub(crate) fn new(
        name: impl Into<Cow<'static, str>>,
        inspect_parent: Arc<fuchsia_inspect::Node>,
    ) -> Self {
        Self::new_set_persistence(name, inspect_parent, false)
    }

    fn new_set_persistence(
        name: impl Into<Cow<'static, str>>,
        inspect_parent: Arc<fuchsia_inspect::Node>,
        persistent: bool,
    ) -> Self {
        let cow_name = name.into();
        Self {
            inner: Arc::new(DiagnosticNodeInner {
                inspect: inspect_parent.create_child(cow_name.deref()),
                parent: Parent::Root(inspect_parent),
                name: cow_name,
                properties: Mutex::new(HashMap::new()),
                persistent,
            }),
        }
    }

    /// Create a |DiagnosticNode| as a child of &self.
    pub(crate) fn child(&self, name: impl Into<Cow<'static, str>>) -> Self {
        let cow_name = name.into();
        Self {
            inner: Arc::new(DiagnosticNodeInner {
                inspect: self.inner.inspect.create_child(cow_name.deref()),
                name: cow_name,
                properties: Mutex::new(HashMap::new()),
                parent: Parent::Node(self.inner.clone()),
                persistent: self.inner.persistent,
            }),
        }
    }

    /// Set a property. If the key is already in use, overrides the value.
    pub(crate) fn set_property(&self, key: &'static str, value: impl Into<Cow<'static, str>>) {
        let mut prop_lock = self.inner.properties.lock();
        let cow_val = value.into();
        match prop_lock.entry(key) {
            Entry::Occupied(mut entry) => {
                let val = entry.get_mut();
                val.1.set(cow_val.deref());
                val.0 = cow_val;
            }
            Entry::Vacant(entry) => {
                let prop = self.inner.inspect.create_string(key, cow_val.deref());
                entry.insert((cow_val, prop));
            }
        }
    }

    /// Mark a property as true.
    pub(crate) fn set_flag(&self, key: &'static str) {
        self.set_property(key, "true")
    }

    fn ancestors_and_self(&self) -> Vec<Arc<DiagnosticNodeInner>> {
        let mut ancestors = vec![self.inner.clone()];
        let mut next_parent = self.inner.parent.clone();
        while let Parent::Node(parent) = next_parent.clone() {
            next_parent = parent.parent.clone();
            ancestors.push(parent);
        }
        ancestors.reverse();
        ancestors
    }
}

impl std::ops::Drop for DiagnosticNodeInner {
    fn drop(&mut self) {
        if self.persistent {
            let parent_inspect = match &self.parent {
                Parent::Node(inner) => &inner.inspect,
                Parent::Root(inspect) => &*inspect,
            };
            let inspect = std::mem::take(&mut self.inspect);
            for (_, val) in self.properties.lock().drain() {
                inspect.record(val.1);
            }
            inspect.record_bool("_dropped", true);
            parent_inspect.record(inspect);
        }
    }
}

impl Debug for DiagnosticNode {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let ancestors = self.ancestors_and_self();
        f.debug_list().entries(ancestors).finish()
    }
}

impl Debug for DiagnosticNodeInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let mut debug_struct = f.debug_struct(self.name.deref());
        for (key, value) in self.properties.lock().iter() {
            debug_struct.field(key, &value.0.deref());
        }
        debug_struct.finish()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn inspect_lifetimes() {
        let inspector = Inspector::default();
        let root_node = DiagnosticNode::new("root", Arc::new(inspector.root().clone_weak()));

        assert_data_tree!(
            inspector,
            root: {
                root: {}
            }
        );

        root_node.set_property("property", "value");

        assert_data_tree!(
            inspector,
            root: {
                root: {
                    property: "value",
                }
            }
        );

        let child_node = root_node.child("child");
        assert_data_tree!(
            inspector,
            root: {
                root: {
                    property: "value",
                    child: {}
                }
            }
        );

        drop(child_node);
        assert_data_tree!(
            inspector,
            root: {
                root: {
                    property: "value",
                }
            }
        );

        drop(root_node);
        assert_data_tree!(
            inspector,
            root: {}
        );
    }

    #[fuchsia::test]
    fn debug_fmt_hierarchy() {
        let inspector = Inspector::default();
        let root_node = DiagnosticNode::new("root", Arc::new(inspector.root().clone_weak()));

        assert!(format!("{:?}", root_node).contains("root"));

        let child_node = root_node.child("child");

        // child should display parent too.
        assert!(format!("{:?}", child_node).contains("child"));
        assert!(format!("{:?}", child_node).contains("root"));

        // grandchild should display all ancestors.
        let grandchild = child_node.child("grand");
        assert!(format!("{:?}", grandchild).contains("grand"));
        assert!(format!("{:?}", grandchild).contains("child"));
        assert!(format!("{:?}", grandchild).contains("root"));

        // descendants still print ancestors even if they are dropped
        drop(root_node);
        drop(child_node);
        assert!(format!("{:?}", grandchild).contains("grand"));
        assert!(format!("{:?}", grandchild).contains("child"));
        assert!(format!("{:?}", grandchild).contains("root"));
    }

    #[fuchsia::test]
    fn debug_fmt_properties() {
        let inspector = Inspector::default();
        let root_node = DiagnosticNode::new("root", Arc::new(inspector.root().clone_weak()));

        assert!(format!("{:?}", root_node).contains("root"));

        root_node.set_property("property", "value");
        assert!(format!("{:?}", root_node).contains("root"));
        assert!(format!("{:?}", root_node).contains("property"));
        assert!(format!("{:?}", root_node).contains("value"));
    }
}
