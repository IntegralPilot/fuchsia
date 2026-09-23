// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::private::InspectTypeInternal;
use crate::writer::{
    BoolProperty, BytesProperty, DoubleArrayProperty, DoubleExponentialHistogramProperty,
    DoubleLinearHistogramProperty, DoubleProperty, Error, Inner, InnerData, InnerType, InspectType,
    InspectTypeReparentable, Inspector, IntArrayProperty, IntExponentialHistogramProperty,
    IntLinearHistogramProperty, IntProperty, LazyNode, State, StringArrayProperty, StringProperty,
    UintArrayProperty, UintExponentialHistogramProperty, UintLinearHistogramProperty, UintProperty,
    ValueList,
};
use diagnostics_hierarchy::{ArrayFormat, ExponentialHistogramParams, LinearHistogramParams};
use futures::future::{BoxFuture, LocalBoxFuture};
use futures::task::{Context, Poll};
use inspect_format::{BlockIndex, LinkNodeDisposition};
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

/// A wrapper for `LocalBoxFuture` that implements `Send`.
///
/// This is only safe to use in contexts where the future is guaranteed to be polled
/// on the same thread it was created on. `fragile::Fragile` will panic if accessed
/// from a different thread. This is useful for allowing `LocalBoxFuture` within
/// closures that require `Send + Sync`, such as those used in `create_lazy_child`.
struct SendFuture {
    inner: fragile::Fragile<LocalBoxFuture<'static, Result<Inspector, anyhow::Error>>>,
}

impl futures::Future for SendFuture {
    type Output = Result<Inspector, anyhow::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut pinned = std::pin::pin!(self.inner.get_mut());
        pinned.as_mut().poll(cx)
    }
}

/// Inspect Node data type.
///
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct Node {
    pub(crate) inner: Inner<InnerNodeType>,
}

impl InspectType for Node {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::Node(self)
    }
}

crate::impl_inspect_type_internal!(Node);

impl Node {
    /// Create a weak reference to the original node. All operations on a weak
    /// reference have identical semantics to the original node for as long
    /// as the original node is live. After that, all operations are no-ops.
    pub fn clone_weak(&self) -> Node {
        Self { inner: self.inner.clone_weak() }
    }

    /// Add a child to this node.
    #[must_use]
    pub fn create_child<'a>(&self, name: impl Into<Cow<'a, str>>) -> Node {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| state.create_node(name.into(), inner_ref.block_index))
                    .map(|block_index| Node::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(Node::new_no_op)
    }

    /// Creates and keeps track of a child with the given `name`.
    pub fn record_child<'a, F>(&self, name: impl Into<Cow<'a, str>>, initialize: F)
    where
        F: FnOnce(&Node),
    {
        self.atomic_update(move |n| {
            let child = n.create_child(name.into());
            initialize(&child);
            n.record(child);
        });
    }

    /// Takes a function to execute as under a single lock of the Inspect VMO. This function
    /// receives a reference to the `Node` where this is called.
    pub fn atomic_update<F, R>(&self, update_fn: F) -> R
    where
        F: FnOnce(&Node) -> R,
    {
        self.atomic_access(update_fn)
    }

    /// Keeps track of the given property for the lifetime of the node.
    pub fn record(&self, property: impl InspectType + 'static) {
        if let Some(inner_ref) = self.inner.inner_ref() {
            inner_ref.data.values.record(property);
        }
    }

    /// Drop all recorded data from the node.
    pub fn clear_recorded(&self) {
        if let Some(inner_ref) = self.inner.inner_ref() {
            inner_ref.data.values.clear();
        }
    }

    /// Creates a new `IntProperty` with the given `name` and `value`.
    #[must_use]
    pub fn create_int<'a>(&self, name: impl Into<Cow<'a, str>>, value: i64) -> IntProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_int_metric(name.into(), value, inner_ref.block_index)
                    })
                    .map(|block_index| IntProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(IntProperty::new_no_op)
    }

    /// Records a new `IntProperty` with the given `name` and `value`.
    pub fn record_int<'a>(&self, name: impl Into<Cow<'a, str>>, value: i64) {
        let property = self.create_int(name.into(), value);
        self.record(property);
    }

    /// Creates a new `UintProperty` with the given `name` and `value`.
    #[must_use]
    pub fn create_uint<'a>(&self, name: impl Into<Cow<'a, str>>, value: u64) -> UintProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_uint_metric(name.into(), value, inner_ref.block_index)
                    })
                    .map(|block_index| UintProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(UintProperty::new_no_op)
    }

    /// Records a new `UintProperty` with the given `name` and `value`.
    pub fn record_uint<'a>(&self, name: impl Into<Cow<'a, str>>, value: u64) {
        let property = self.create_uint(name.into(), value);
        self.record(property);
    }

    /// Creates a new `DoubleProperty` with the given `name` and `value`.
    #[must_use]
    pub fn create_double<'a>(&self, name: impl Into<Cow<'a, str>>, value: f64) -> DoubleProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_double_metric(name.into(), value, inner_ref.block_index)
                    })
                    .map(|block_index| DoubleProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(DoubleProperty::new_no_op)
    }

    /// Records a new `DoubleProperty` with the given `name` and `value`.
    pub fn record_double<'a>(&self, name: impl Into<Cow<'a, str>>, value: f64) {
        let property = self.create_double(name.into(), value);
        self.record(property);
    }

    /// Creates a new `StringArrayProperty` with the given `name` and `slots`.
    #[must_use]
    pub fn create_string_array<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
    ) -> StringArrayProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_string_array(name.into(), slots, inner_ref.block_index)
                    })
                    .map(|block_index| {
                        StringArrayProperty::new(inner_ref.state.clone(), block_index)
                    })
                    .ok()
            })
            .unwrap_or_else(StringArrayProperty::new_no_op)
    }

    /// Creates a new `IntArrayProperty` with the given `name` and `slots`.
    #[must_use]
    pub fn create_int_array<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
    ) -> IntArrayProperty {
        self.create_int_array_internal(name.into(), slots, ArrayFormat::Default)
    }

    #[must_use]
    pub(crate) fn create_int_array_internal<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
        format: ArrayFormat,
    ) -> IntArrayProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_int_array(name.into(), slots, format, inner_ref.block_index)
                    })
                    .map(|block_index| IntArrayProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(IntArrayProperty::new_no_op)
    }

    /// Creates a new `UintArrayProperty` with the given `name` and `slots`.
    #[must_use]
    pub fn create_uint_array<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
    ) -> UintArrayProperty {
        self.create_uint_array_internal(name.into(), slots, ArrayFormat::Default)
    }

    #[must_use]
    pub(crate) fn create_uint_array_internal<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
        format: ArrayFormat,
    ) -> UintArrayProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_uint_array(name.into(), slots, format, inner_ref.block_index)
                    })
                    .map(|block_index| UintArrayProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(UintArrayProperty::new_no_op)
    }

    /// Creates a new `DoubleArrayProperty` with the given `name` and `slots`.
    #[must_use]
    pub fn create_double_array<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
    ) -> DoubleArrayProperty {
        self.create_double_array_internal(name.into(), slots, ArrayFormat::Default)
    }

    #[must_use]
    pub(crate) fn create_double_array_internal<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        slots: usize,
        format: ArrayFormat,
    ) -> DoubleArrayProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_double_array(name.into(), slots, format, inner_ref.block_index)
                    })
                    .map(|block_index| {
                        DoubleArrayProperty::new(inner_ref.state.clone(), block_index)
                    })
                    .ok()
            })
            .unwrap_or_else(DoubleArrayProperty::new_no_op)
    }

    /// Creates a new `IntLinearHistogramProperty` with the given `name` and `params`.
    #[must_use]
    pub fn create_int_linear_histogram<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        params: LinearHistogramParams<i64>,
    ) -> IntLinearHistogramProperty {
        IntLinearHistogramProperty::new(name.into(), params, self)
    }

    /// Creates a new `UintLinearHistogramProperty` with the given `name` and `params`.
    #[must_use]
    pub fn create_uint_linear_histogram<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        params: LinearHistogramParams<u64>,
    ) -> UintLinearHistogramProperty {
        UintLinearHistogramProperty::new(name.into(), params, self)
    }

    /// Creates a new `DoubleLinearHistogramProperty` with the given `name` and `params`.
    #[must_use]
    pub fn create_double_linear_histogram<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        params: LinearHistogramParams<f64>,
    ) -> DoubleLinearHistogramProperty {
        DoubleLinearHistogramProperty::new(name.into(), params, self)
    }

    /// Creates a new `IntExponentialHistogramProperty` with the given `name` and `params`.
    #[must_use]
    pub fn create_int_exponential_histogram<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        params: ExponentialHistogramParams<i64>,
    ) -> IntExponentialHistogramProperty {
        IntExponentialHistogramProperty::new(name.into(), params, self)
    }

    /// Creates a new `UintExponentialHistogramProperty` with the given `name` and `params`.
    #[must_use]
    pub fn create_uint_exponential_histogram<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        params: ExponentialHistogramParams<u64>,
    ) -> UintExponentialHistogramProperty {
        UintExponentialHistogramProperty::new(name.into(), params, self)
    }

    /// Creates a new `DoubleExponentialHistogramProperty` with the given `name` and `params`.
    #[must_use]
    pub fn create_double_exponential_histogram<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        params: ExponentialHistogramParams<f64>,
    ) -> DoubleExponentialHistogramProperty {
        DoubleExponentialHistogramProperty::new(name.into(), params, self)
    }

    /// Creates a new lazy child with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    #[must_use]
    pub fn create_lazy_child<'a, F>(&self, name: impl Into<Cow<'a, str>>, callback: F) -> LazyNode
    where
        F: Fn() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> + Sync + Send + 'static,
    {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_lazy_node(
                            name.into(),
                            inner_ref.block_index,
                            LinkNodeDisposition::Child,
                            callback,
                        )
                    })
                    .map(|block_index| LazyNode::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(LazyNode::new_no_op)
    }

    /// Records a new lazy child with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    pub fn record_lazy_child<'a, F>(&self, name: impl Into<Cow<'a, str>>, callback: F)
    where
        F: Fn() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> + Sync + Send + 'static,
    {
        let property = self.create_lazy_child(name.into(), callback);
        self.record(property);
    }

    /// Creates a new lazy child with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    ///
    /// The inspector referenced by this function should not be sent between threads,
    /// or it may panic.
    #[must_use]
    pub fn create_lazy_child_with_thread_local<'a, F>(
        &self,
        name: impl Into<Cow<'a, str>>,
        callback: F,
    ) -> LazyNode
    where
        F: Fn() -> LocalBoxFuture<'static, Result<Inspector, anyhow::Error>> + 'static,
    {
        let fragile_callback = fragile::Fragile::new(callback);
        self.create_lazy_child(name, move || {
            let cb = fragile_callback.get();
            Box::pin(SendFuture { inner: fragile::Fragile::new(cb()) })
        })
    }

    /// Records a new lazy child with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    ///
    /// The inspector referenced by this function should not be sent between threads,
    /// or it may panic.
    pub fn record_lazy_child_with_thread_local<'a, F>(
        &self,
        name: impl Into<Cow<'a, str>>,
        callback: F,
    ) where
        F: Fn() -> LocalBoxFuture<'static, Result<Inspector, anyhow::Error>> + 'static,
    {
        let property = self.create_lazy_child_with_thread_local(name.into(), callback);
        self.record(property);
    }

    /// Creates a new inline lazy node with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    #[must_use]
    pub fn create_lazy_values<'a, F>(&self, name: impl Into<Cow<'a, str>>, callback: F) -> LazyNode
    where
        F: Fn() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> + Sync + Send + 'static,
    {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_lazy_node(
                            name.into(),
                            inner_ref.block_index,
                            LinkNodeDisposition::Inline,
                            callback,
                        )
                    })
                    .map(|block_index| LazyNode::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(LazyNode::new_no_op)
    }

    /// Records a new inline lazy node with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    pub fn record_lazy_values<'a, F>(&self, name: impl Into<Cow<'a, str>>, callback: F)
    where
        F: Fn() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> + Sync + Send + 'static,
    {
        let property = self.create_lazy_values(name.into(), callback);
        self.record(property);
    }

    /// Creates a new inline lazy node with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    ///
    /// The inspector referenced by this function should not be sent between threads,
    /// or it may panic.
    #[must_use]
    pub fn create_lazy_values_with_thread_local<'a, F>(
        &self,
        name: impl Into<Cow<'a, str>>,
        callback: F,
    ) -> LazyNode
    where
        F: Fn() -> LocalBoxFuture<'static, Result<Inspector, anyhow::Error>> + 'static,
    {
        let fragile_callback = fragile::Fragile::new(callback);
        self.create_lazy_values(name, move || {
            let cb = fragile_callback.get();
            Box::pin(SendFuture { inner: fragile::Fragile::new(cb()) })
        })
    }

    /// Records a new inline lazy node with the given `name` and `callback`.
    /// `callback` will be invoked each time the component's Inspect is read.
    /// `callback` is expected to create a new Inspector and return it;
    /// its contents will be rooted at the intended location (the `self` node).
    ///
    /// The inspector referenced by this function should not be sent between threads,
    /// or it may panic.
    pub fn record_lazy_values_with_thread_local<'a, F>(
        &self,
        name: impl Into<Cow<'a, str>>,
        callback: F,
    ) where
        F: Fn() -> LocalBoxFuture<'static, Result<Inspector, anyhow::Error>> + 'static,
    {
        let property = self.create_lazy_values_with_thread_local(name.into(), callback);
        self.record(property);
    }

    /// Add a string property to this node.
    #[must_use]
    pub fn create_string<'a, 'b>(
        &self,
        name: impl Into<Cow<'a, str>>,
        value: impl Into<Cow<'b, str>>,
    ) -> StringProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_string(name.into(), value.into(), inner_ref.block_index)
                    })
                    .map(|block_index| StringProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(StringProperty::new_no_op)
    }

    /// Creates and saves a string property for the lifetime of the node.
    pub fn record_string<'a, 'b>(
        &self,
        name: impl Into<Cow<'a, str>>,
        value: impl Into<Cow<'b, str>>,
    ) {
        let property = self.create_string(name, value);
        self.record(property);
    }

    /// Add a byte vector property to this node.
    #[must_use]
    pub fn create_bytes<'a>(
        &self,
        name: impl Into<Cow<'a, str>>,
        value: impl AsRef<[u8]>,
    ) -> BytesProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_buffer_property(
                            name.into(),
                            value.as_ref(),
                            inner_ref.block_index,
                        )
                    })
                    .map(|block_index| BytesProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(BytesProperty::new_no_op)
    }

    /// Creates and saves a bytes property for the lifetime of the node.
    pub fn record_bytes<'a>(&self, name: impl Into<Cow<'a, str>>, value: impl AsRef<[u8]>) {
        let property = self.create_bytes(name.into(), value);
        self.record(property);
    }

    /// Add a bool property to this node.
    #[must_use]
    pub fn create_bool<'a>(&self, name: impl Into<Cow<'a, str>>, value: bool) -> BoolProperty {
        self.inner
            .inner_ref()
            .and_then(|inner_ref| {
                inner_ref
                    .state
                    .try_lock()
                    .and_then(|mut state| {
                        state.create_bool(name.into(), value, inner_ref.block_index)
                    })
                    .map(|block_index| BoolProperty::new(inner_ref.state.clone(), block_index))
                    .ok()
            })
            .unwrap_or_else(BoolProperty::new_no_op)
    }

    /// Creates and saves a bool property for the lifetime of the node.
    pub fn record_bool<'a>(&self, name: impl Into<Cow<'a, str>>, value: bool) {
        let property = self.create_bool(name.into(), value);
        self.record(property);
    }

    /// Takes a child from its parent and adopts it into its own tree.
    pub fn adopt<T: InspectTypeReparentable>(&self, child: &T) -> Result<(), Error> {
        child.reparent(self)
    }

    /// Removes this node from the Inspect tree. Typically, just dropping the Node must be
    /// preferred. However, this is a convenience method meant for power user implementations that
    /// need more control over the lifetime of a Node. For example, by being able to remove the node
    /// from a weak clone of it.
    pub fn forget(&self) {
        if let Some(inner_ref) = self.inner.inner_ref() {
            let _ = InnerNodeType::free(&inner_ref.state, &inner_ref.data, inner_ref.block_index);
        }
    }

    /// Creates a new root node.
    pub(crate) fn new_root(state: State) -> Node {
        Node::new(state, BlockIndex::ROOT)
    }
}

#[derive(Default, Debug)]
pub(crate) struct InnerNodeType;

#[derive(Default, Debug)]
pub(crate) struct NodeData {
    values: ValueList,
    destroyed: AtomicBool,
}

impl InnerData for NodeData {
    fn is_valid(&self) -> bool {
        !self.destroyed.load(Ordering::SeqCst)
    }
}

impl InnerType for InnerNodeType {
    // Each node has a list of recorded values.
    type Data = NodeData;

    fn free(state: &State, data: &Self::Data, block_index: BlockIndex) -> Result<(), Error> {
        if block_index == BlockIndex::ROOT {
            return Ok(());
        }
        let mut state_lock = state.try_lock()?;
        if data.destroyed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        state_lock.free_value(block_index).map_err(|err| Error::free("node", block_index, err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader;
    use crate::writer::ArrayProperty;
    use crate::writer::testing_utils::{GetBlockExt, get_state};
    use diagnostics_assertions::{assert_data_tree, assert_json_diff};
    use futures::FutureExt;
    use inspect_format::BlockType;

    #[fuchsia::test]
    fn node() {
        // Create and use a default value.
        let default = Node::default();
        default.record_int("a", 0);

        let state = get_state(4096);
        let root = Node::new_root(state);
        let node = root.create_child("node");
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.block_type(), Some(BlockType::NodeValue));
            assert_eq!(node_block.child_count(), 0);
        });
        {
            let child = node.create_child("child");
            child.get_block::<_, inspect_format::Node>(|child_block| {
                assert_eq!(child_block.block_type(), Some(BlockType::NodeValue));
                assert_eq!(child_block.child_count(), 0);
            });
            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 1);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }

    #[fuchsia::test]
    async fn lazy_child() {
        let inspector = Inspector::default();
        let _lazy = inspector.root().create_lazy_child("lazy-1", || {
            async move {
                let insp = Inspector::default();
                insp.root().record_lazy_child("parent", || {
                    async move {
                        let insp2 = Inspector::default();
                        insp2.root().record_int("create-lazy-child", 0);
                        insp2.root().record_int("create-lazy-child-2", 2);
                        Ok(insp2)
                    }
                    .boxed()
                });
                Ok(insp)
            }
            .boxed()
        });

        inspector.root().record_lazy_child("lazy-2", || {
            async move {
                let insp = Inspector::default();
                insp.root().record_bool("recorded-lazy-child", true);
                Ok(insp)
            }
            .boxed()
        });

        inspector.root().record_lazy_values("lazy", || {
            async move {
                let insp = Inspector::default();
                insp.root().record_bool("recorded-lazy-values", true);
                Ok(insp)
            }
            .boxed()
        });

        let result = reader::read(&inspector).await.unwrap();

        assert_data_tree!(result, root: {
            "lazy-1": {
                "parent": {
                    "create-lazy-child": 0i64,
                    "create-lazy-child-2": 2i64,
                },
            },
            "lazy-2": {
                "recorded-lazy-child": true,
            },
            "recorded-lazy-values": true,
        });
    }

    #[fuchsia::test]
    async fn lazy_child_with_thread_local() {
        let inspector = Inspector::default();
        inspector.root().record_lazy_child_with_thread_local("lazy-1", || {
            async move {
                let insp = Inspector::default();
                insp.root().record_int("a", 1i64);
                Ok(insp)
            }
            .boxed()
        });

        let result = reader::read(&inspector).await.unwrap();
        assert_data_tree!(result, root: {
            "lazy-1": {
                "a": 1i64,
            },
        });
    }

    #[fuchsia::test]
    async fn lazy_values_with_thread_local() {
        let inspector = Inspector::default();
        inspector.root().record_lazy_values_with_thread_local("lazy-1", || {
            async move {
                let insp = Inspector::default();
                insp.root().record_int("a", 1i64);
                Ok(insp)
            }
            .boxed()
        });

        let result = reader::read(&inspector).await.unwrap();
        assert_data_tree!(result, root: {
            "a": 1i64,
        });
    }

    #[fuchsia::test]
    async fn test_adoption() {
        let insp = Inspector::default();
        let root = insp.root();
        let a = root.create_child("a");
        let b = root.create_child("b");
        let c = b.create_child("c");

        assert_data_tree!(insp, root: {
            a: {},
            b: {
                c: {},
            },
        });

        a.adopt(&b).unwrap();

        assert_data_tree!(insp, root: {
            a: {
                b: {
                    c: {},
                },
            },
        });

        assert!(c.adopt(&a).is_err());
        assert!(c.adopt(&b).is_err());
        assert!(b.adopt(&a).is_err());
        assert!(a.adopt(root).is_err());
        assert!(a.adopt(&a).is_err());

        {
            let d = root.create_int("d", 4);

            assert_data_tree!(insp, root: {
                a: {
                    b: {
                        c: {},
                    },
                },
                d: 4i64,
            });

            c.adopt(&d).unwrap();

            assert_data_tree!(insp, root: {
                a: {
                    b: {
                        c: {
                            d: 4i64,
                        },
                    },
                },
            });
        }

        assert_data_tree!(insp, root: {
            a: {
                b: {
                    c: {},
                },
            },
        });
    }

    #[fuchsia::test]
    fn node_no_op_clone_weak() {
        let default = Node::default();
        assert!(!default.is_valid());
        let weak = default.clone_weak();
        assert!(!weak.is_valid());
        let _ = weak.create_child("child");
        std::mem::drop(default);
        let _ = weak.create_uint("age", 1337);
        assert!(!weak.is_valid());
    }

    #[fuchsia::test]
    fn node_clone_weak() {
        let state = get_state(4096);
        let root = Node::new_root(state);
        let node = root.create_child("node");
        let node_weak = node.clone_weak();
        let node_weak_2 = node_weak.clone_weak(); // Weak from another weak

        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.block_type(), Some(BlockType::NodeValue));
            assert_eq!(node_block.child_count(), 0);
        });

        let child_from_strong = node.create_child("child");
        let child = node_weak.create_child("child_1");
        let child_2 = node_weak_2.create_child("child_2");
        std::mem::drop(node_weak_2);
        node.get_block::<_, inspect_format::Node>(|block| {
            assert_eq!(block.child_count(), 3);
        });
        std::mem::drop(child_from_strong);
        node.get_block::<_, inspect_format::Node>(|block| {
            assert_eq!(block.child_count(), 2);
        });
        std::mem::drop(child);
        node.get_block::<_, inspect_format::Node>(|block| {
            assert_eq!(block.child_count(), 1);
        });
        assert!(node_weak.is_valid());
        assert!(child_2.is_valid());
        std::mem::drop(node);
        assert!(!node_weak.is_valid());
        let _ = node_weak.create_child("orphan");
        let _ = child_2.create_child("orphan");
    }

    #[fuchsia::test]
    fn dummy_partialeq() {
        let inspector = Inspector::default();
        let root = inspector.root();

        // Types should all be equal to another type. This is to enable clients
        // with inspect types in their structs be able to derive PartialEq and
        // Eq smoothly.
        assert_eq!(root, &root.create_child("child1"));
        assert_eq!(root.create_int("property1", 1), root.create_int("property2", 2));
        assert_eq!(root.create_double("property1", 1.0), root.create_double("property2", 2.0));
        assert_eq!(root.create_uint("property1", 1), root.create_uint("property2", 2));
        assert_eq!(
            root.create_string("property1", "value1"),
            root.create_string("property2", "value2")
        );
        assert_eq!(
            root.create_bytes("property1", b"value1"),
            root.create_bytes("property2", b"value2")
        );
    }

    #[fuchsia::test]
    async fn record() {
        let inspector = Inspector::default();
        let property = inspector.root().create_uint("a", 1);
        inspector.root().record_uint("b", 2);
        {
            let child = inspector.root().create_child("child");
            child.record(property);
            child.record_double("c", 3.25);
            assert_data_tree!(inspector, root: {
                a: 1u64,
                b: 2u64,
                child: {
                    c: 3.25,
                }
            });
        }
        // `child` went out of scope, meaning it was deleted.
        // Property `a` should be gone as well, given that it was being tracked by `child`.
        assert_data_tree!(inspector, root: {
            b: 2u64,
        });

        inspector.root().clear_recorded();
        assert_data_tree!(inspector, root: {});
    }

    #[fuchsia::test]
    async fn clear_recorded() {
        let inspector = Inspector::default();
        let one = inspector.root().create_child("one");
        let two = inspector.root().create_child("two");
        let one_recorded = one.create_child("one_recorded");
        let two_recorded = two.create_child("two_recorded");

        one.record(one_recorded);
        two.record(two_recorded);

        assert_json_diff!(inspector, root: {
            one: {
                one_recorded: {},
            },
            two: {
                two_recorded: {},
            },
        });

        two.clear_recorded();

        assert_json_diff!(inspector, root: {
            one: {
                one_recorded: {},
            },
            two: {},
        });

        one.clear_recorded();

        assert_json_diff!(inspector, root: {
            one: {},
            two: {},
        });
    }

    #[fuchsia::test]
    async fn record_child() {
        let inspector = Inspector::default();
        inspector.root().record_child("test", |node| {
            node.record_int("a", 1);
        });
        assert_data_tree!(inspector, root: {
            test: {
                a: 1i64,
            }
        })
    }

    #[fuchsia::test]
    async fn record_weak() {
        let inspector = Inspector::default();
        let main = inspector.root().create_child("main");
        let main_weak = main.clone_weak();
        let property = main_weak.create_uint("a", 1);

        // Ensure either the weak or strong reference can be used for recording
        main_weak.record_uint("b", 2);
        main.record_uint("c", 3);
        {
            let child = main_weak.create_child("child");
            child.record(property);
            child.record_double("c", 3.25);
            assert_data_tree!(inspector, root: { main: {
                a: 1u64,
                b: 2u64,
                c: 3u64,
                child: {
                    c: 3.25,
                }
            }});
        }
        // `child` went out of scope, meaning it was deleted.
        // Property `a` should be gone as well, given that it was being tracked by `child`.
        assert_data_tree!(inspector, root: { main: {
            b: 2u64,
            c: 3u64
        }});
        std::mem::drop(main);
        // Recording after dropping a strong reference is a no-op
        main_weak.record_double("d", 1.0);
        // Verify that dropping a strong reference cleans up the state
        assert_data_tree!(inspector, root: { });
    }

    #[fuchsia::test]
    async fn string_arrays_on_record() {
        let inspector = Inspector::default();
        inspector.root().record_child("child", |node| {
            node.record_int("my_int", 1i64);

            let arr: crate::StringArrayProperty = node.create_string_array("my_string_array", 1);
            arr.set(0, "test");
            node.record(arr);
        });
        assert_data_tree!(inspector, root: {
            child: {
                my_int: 1i64,
                my_string_array: vec!["test"]
            }
        });
    }

    #[fuchsia::test]
    async fn we_can_delete_a_node_explicitly_with_the_weak_clone() {
        let insp = Inspector::default();
        let a = insp.root().create_child("a");
        let _property = a.create_int("foo", 1);
        assert_data_tree!(insp, root: {
            a: {
                foo: 1i64,
            }
        });

        let a_weak = a.clone_weak();
        a_weak.forget();
        assert!(a.inner.inner_ref().is_none());
        assert_data_tree!(insp, root: {});
    }
}

// Tests that either refer explicitly to VMOs or utilize zircon signals.
#[cfg(all(test, target_os = "fuchsia"))]
mod fuchsia_tests {
    use super::*;
    use crate::hierarchy::DiagnosticsHierarchy;
    use crate::{NumericProperty, reader};
    use diagnostics_assertions::assert_json_diff;
    use std::sync::Arc;
    use zx::Peered;

    #[fuchsia::test]
    fn atomic_update_reader() {
        let inspector = Inspector::default();

        // Spawn a read thread that holds a duplicate handle to the VMO that will be written.
        let vmo = Arc::new(inspector.duplicate_vmo().expect("duplicate vmo handle"));
        let (p1, p2) = zx::EventPair::create();

        macro_rules! notify_and_wait_reader {
            () => {
                p1.signal_peer(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
                p1.wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE).unwrap();
                p1.signal(zx::Signals::USER_0, zx::Signals::NONE).unwrap();
            };
        }

        macro_rules! wait_and_notify_writer {
            ($code:block) => {
              p2.wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE).unwrap();
              p2.signal(zx::Signals::USER_0, zx::Signals::NONE).unwrap();
              $code
              p2.signal_peer(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
            }
        }

        let thread = std::thread::spawn(move || {
            // Before running the atomic update.
            wait_and_notify_writer! {{
                let hierarchy: DiagnosticsHierarchy<String> =
                    reader::PartialNodeHierarchy::try_from(vmo.as_ref()).unwrap().into();
                assert_eq!(hierarchy, DiagnosticsHierarchy::new_root());
            }};
            // After: create_child("child"): Assert that the VMO is in use (locked) and we can't
            // read.
            wait_and_notify_writer! {{
                assert!(reader::PartialNodeHierarchy::try_from(vmo.as_ref()).is_err());
            }};
            // After: record_int("a"): Assert that the VMO is in use (locked) and we can't
            // read.
            wait_and_notify_writer! {{
                assert!(reader::PartialNodeHierarchy::try_from(vmo.as_ref()).is_err());
            }};
            // After: record_int("b"): Assert that the VMO is in use (locked) and we can't
            // read.
            wait_and_notify_writer! {{
                assert!(reader::PartialNodeHierarchy::try_from(vmo.as_ref()).is_err());
            }};
            // After atomic update
            wait_and_notify_writer! {{
                let hierarchy: DiagnosticsHierarchy<String> =
                    reader::PartialNodeHierarchy::try_from(vmo.as_ref()).unwrap().into();
                // Attempting to make this whole lambda async and using Scope or similar
                // results in https://github.com/rust-lang/rust/issues/100013.
                fuchsia_async::TestExecutor::new().run_singlethreaded(async move {
                    assert_json_diff!(hierarchy, root: {
                       value: 2i64,
                       child: {
                           a: 1i64,
                           b: 2i64,
                       }
                    });
                });
            }};
        });

        // Perform the atomic update
        let mut child = Node::default();
        notify_and_wait_reader!();
        let int_val = inspector.root().create_int("value", 1);
        inspector
            .root()
            .atomic_update(|node| {
                // Intentionally make this slow to assert an atomic update in the reader.
                child = node.create_child("child");
                notify_and_wait_reader!();
                child.record_int("a", 1);
                notify_and_wait_reader!();
                child.record_int("b", 2);
                notify_and_wait_reader!();
                int_val.add(1);
                Ok::<(), Error>(())
            })
            .expect("successful atomic update");
        notify_and_wait_reader!();

        // Wait for the reader thread to successfully finish.
        let _ = thread.join();

        // Ensure that the variable that we mutated internally can be used.
        child.record_int("c", 3);
        fuchsia_async::TestExecutor::new().run_singlethreaded(async move {
            assert_json_diff!(inspector, root: {
                value: 2i64,
                child: {
                    a: 1i64,
                    b: 2i64,
                    c: 3i64,
                }
            });
        });
    }
}
