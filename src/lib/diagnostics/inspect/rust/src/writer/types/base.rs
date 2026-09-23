// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{Error, Node, State};
use derivative::Derivative;
use inspect_format::BlockIndex;
use private::InspectTypeInternal;
use std::fmt::Debug;
use std::sync::{Arc, Weak};

/// Trait implemented by all inspect types.
pub trait InspectType: Send + Sync + Debug {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType
    where
        Self: Sized + 'static;
}

pub(crate) mod private {
    use crate::writer::State;
    use inspect_format::BlockIndex;

    /// Trait implemented by all inspect types. It provides constructor functions that are not
    /// intended for use outside the crate.
    /// Use `impl_inspect_type_internal` for easy implementation.
    pub trait InspectTypeInternal {
        fn new(state: State, block_index: BlockIndex) -> Self;
        fn new_no_op() -> Self;
        fn is_valid(&self) -> bool;
        fn block_index(&self) -> Option<BlockIndex>;
        fn state(&self) -> Option<State>;
        fn atomic_access<R, F: FnOnce(&Self) -> R>(&self, accessor: F) -> R;
    }
}

/// Trait allowing a `Node` to adopt any Inspect type as its child, removing
/// it from the original parent's tree.
///
/// This trait is not implementable by external types.
pub trait InspectTypeReparentable: private::InspectTypeInternal {
    #[doc(hidden)]
    /// This function is called by a child with the new parent as an argument.
    /// The child will be removed from its current parent and added to the tree
    /// under new_parent.
    fn reparent(&self, new_parent: &Node) -> Result<(), Error> {
        if let (
            Some(child_state),
            Some(child_index),
            Some(new_parent_state),
            Some(new_parent_index),
        ) = (self.state(), self.block_index(), new_parent.state(), new_parent.block_index())
        {
            if new_parent_state != child_state {
                return Err(Error::AdoptionIntoWrongVmo);
            }

            new_parent_state
                .try_lock()
                .and_then(|mut state| state.reparent(child_index, new_parent_index))?;
        }

        Ok(())
    }
}

impl<T: private::InspectTypeInternal> InspectTypeReparentable for T {}

/// Macro to generate private::InspectTypeInternal
macro_rules! impl_inspect_type_internal {
    ($type_name:ident) => {
        impl $crate::private::InspectTypeInternal for $type_name {
            fn new(
                state: $crate::writer::State,
                block_index: inspect_format::BlockIndex,
            ) -> $type_name {
                $type_name { inner: $crate::writer::types::base::Inner::new(state, block_index) }
            }

            fn is_valid(&self) -> bool {
                self.inner.is_valid()
            }

            fn new_no_op() -> $type_name {
                $type_name { inner: $crate::writer::types::base::Inner::None }
            }

            fn state(&self) -> Option<$crate::writer::State> {
                Some(self.inner.inner_ref()?.state.clone())
            }

            fn block_index(&self) -> Option<inspect_format::BlockIndex> {
                if let Some(ref inner_ref) = self.inner.inner_ref() {
                    Some(inner_ref.block_index)
                } else {
                    None
                }
            }

            fn atomic_access<R, F: FnOnce(&Self) -> R>(&self, accessor: F) -> R {
                match self.inner.inner_ref() {
                    None => {
                        // If the node was a no-op we still execute the `accessor` even if all
                        // operations inside it will be no-ops to return `R`.
                        accessor(&self)
                    }
                    Some(inner_ref) => {
                        // Silently ignore the error when fail to lock (as in any regular operation).
                        // All operations performed in the `accessor` won't update the vmo
                        // generation count since we'll be holding one lock here.
                        inner_ref.state.begin_transaction();
                        let result = accessor(&self);
                        inner_ref.state.end_transaction();
                        result
                    }
                }
            }
        }
    };
}

pub(crate) use impl_inspect_type_internal;

/// An inner type of all inspect nodes and properties. Each variant implies a
/// different relationship with the underlying inspect VMO.
#[derive(Debug, Derivative)]
#[derivative(Default)]
pub(crate) enum Inner<T: InnerType> {
    /// The node or property is not attached to the inspect VMO.
    #[derivative(Default)]
    None,

    /// The node or property is attached to the inspect VMO, iff its strong
    /// reference is still alive.
    Weak(Weak<InnerRef<T>>),

    /// The node or property is attached to the inspect VMO.
    Strong(Arc<InnerRef<T>>),
}

impl<T: InnerType> Inner<T> {
    /// Creates a new Inner with the desired block index within the inspect VMO
    pub(crate) fn new(state: State, block_index: BlockIndex) -> Self {
        Self::Strong(Arc::new(InnerRef { state, block_index, data: T::Data::default() }))
    }

    /// Returns true if the number of strong references to this node or property
    /// is greater than 0.
    pub(crate) fn is_valid(&self) -> bool {
        match self {
            Self::None => false,
            Self::Weak(weak_ref) => match weak_ref.upgrade() {
                None => false,
                Some(inner_ref) => inner_ref.data.is_valid(),
            },
            Self::Strong(inner_ref) => inner_ref.data.is_valid(),
        }
    }

    /// Returns a `Some(Arc<InnerRef>)` iff the node or property is currently
    /// attached to inspect, or `None` otherwise. Weak pointers are upgraded
    /// if possible, but their lifetime as strong references are expected to be
    /// short.
    pub(crate) fn inner_ref(&self) -> Option<Arc<InnerRef<T>>> {
        match self {
            Self::None => None,
            Self::Weak(weak_ref) => {
                if let Some(inner_ref) = weak_ref.upgrade()
                    && inner_ref.data.is_valid()
                {
                    return Some(inner_ref);
                }
                None
            }
            Self::Strong(inner_ref) => {
                if inner_ref.data.is_valid() {
                    Some(Arc::clone(inner_ref))
                } else {
                    None
                }
            }
        }
    }

    /// Make a weak reference.
    pub(crate) fn clone_weak(&self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Weak(weak_ref) => Self::Weak(weak_ref.clone()),
            Self::Strong(inner_ref) => {
                if inner_ref.data.is_valid() {
                    Self::Weak(Arc::downgrade(inner_ref))
                } else {
                    Self::None
                }
            }
        }
    }
}

/// Inspect API types implement Eq,PartialEq returning true all the time so that
/// structs embedding inspect types can derive these traits as well.
/// IMPORTANT: Do not rely on these traits implementations for real comparisons
/// or validation tests, instead leverage the reader.
impl<T: InnerType> PartialEq for Inner<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T: InnerType> Eq for Inner<T> {}

/// A type that is owned by inspect nodes and properties, sharing ownership of
/// the inspect VMO heap, and with numerical pointers to the location in the
/// heap in which it resides.
#[derive(Debug)]
pub(crate) struct InnerRef<T: InnerType> {
    /// Index of the block in the VMO.
    pub(crate) block_index: BlockIndex,

    /// Reference to the VMO heap.
    pub(crate) state: State,

    /// Associated data for this type.
    pub(crate) data: T::Data,
}

impl<T: InnerType> Drop for InnerRef<T> {
    /// InnerRef has a manual drop impl, to guarantee a single deallocation in
    /// the case of multiple strong references.
    fn drop(&mut self) {
        if let Err(e) = T::free(&self.state, &self.data, self.block_index) {
            log::error!("Failed to free InnerRef: {:?}", e);
        }
    }
}

/// De-allocation behavior and associated data for an inner type.
pub(crate) trait InnerType {
    /// Associated data stored on the InnerRef
    type Data: Default + Debug + InnerData;

    /// De-allocation behavior for when the InnerRef gets dropped
    fn free(state: &State, data: &Self::Data, block_index: BlockIndex) -> Result<(), Error>;
}

pub(crate) trait InnerData {
    fn is_valid(&self) -> bool;
}

impl InnerData for () {
    fn is_valid(&self) -> bool {
        true
    }
}

#[derive(Default, Debug)]
pub(crate) struct InnerValueType;

impl InnerType for InnerValueType {
    type Data = ();
    fn free(state: &State, _: &Self::Data, block_index: BlockIndex) -> Result<(), Error> {
        let mut state_lock = state.try_lock()?;
        state_lock.free_value(block_index).map_err(|err| Error::free("value", block_index, err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inspector;
    use diagnostics_assertions::assert_data_tree;

    #[fuchsia::test]
    async fn test_reparent_from_state() {
        let insp = Inspector::default();
        let root = insp.root();
        let a = root.create_child("a");
        let b = a.create_child("b");

        assert_data_tree!(insp, root: {
            a: {
                b: {},
            },
        });

        b.reparent(root).unwrap();

        assert_data_tree!(insp, root: {
            b: {},
            a: {},
        });
    }

    #[fuchsia::test]
    fn reparent_from_wrong_state() {
        let insp1 = Inspector::default();
        let insp2 = Inspector::default();

        assert!(insp1.root().reparent(insp2.root()).is_err());

        let a = insp1.root().create_child("a");
        let b = insp2.root().create_child("b");

        assert!(a.reparent(&b).is_err());
        assert!(b.reparent(&a).is_err());
    }
}
