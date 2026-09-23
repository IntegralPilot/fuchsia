// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::private::InspectTypeInternal;
use crate::writer::state::Stats;
use crate::writer::{Error, Heap, Node, State};
use diagnostics_hierarchy::{DiagnosticsHierarchy, DiagnosticsHierarchyGetter};
use inspect_format::{BlockContainer, Container, constants};
use log::error;
use std::borrow::Cow;
use std::cmp::max;
use std::sync::Arc;

#[cfg(target_os = "fuchsia")]
use inspect_format::{BlockAccessorExt, Header};

/// Root of the Inspect API. Through this API, further nodes can be created and inspect can be
/// served.
#[derive(Clone)]
pub struct Inspector {
    /// The root node.
    root_node: Arc<Node>,

    /// The storage backing the inspector. This is a VMO when working on Fuchsia.
    #[allow(dead_code)] // unused and meaningless in the host build.
    storage: Option<Arc<<Container as BlockContainer>::ShareableData>>,
}

impl DiagnosticsHierarchyGetter<String> for Inspector {
    async fn get_diagnostics_hierarchy<'a>(&'a self) -> Cow<'_, DiagnosticsHierarchy>
    where
        String: 'a,
    {
        let hierarchy = crate::reader::read(self).await.expect("failed to get hierarchy");
        Cow::Owned(hierarchy)
    }
}

pub trait InspectorIntrospectionExt {
    fn stats(&self) -> Option<Stats>;
}

impl InspectorIntrospectionExt for Inspector {
    fn stats(&self) -> Option<Stats> {
        self.state().and_then(|outer| outer.try_lock().ok().map(|state| state.stats()))
    }
}

#[cfg(target_os = "fuchsia")]
impl Inspector {
    /// Returns a duplicate of the underlying VMO for this Inspector.
    ///
    /// The duplicated VMO will be read-only, and is suitable to send to clients over FIDL.
    pub fn duplicate_vmo(&self) -> Option<zx::Vmo> {
        self.storage.as_ref().and_then(|vmo| {
            vmo.duplicate_handle(
                zx::Rights::BASIC | zx::Rights::READ | zx::Rights::MAP | zx::Rights::GET_PROPERTY,
            )
            .ok()
        })
    }

    /// Returns a duplicate of the underlying VMO for this Inspector with the given rights.
    ///
    /// The duplicated VMO will be read-only, and is suitable to send to clients over FIDL.
    pub fn duplicate_vmo_with_rights(&self, rights: zx::Rights) -> Option<zx::Vmo> {
        self.storage.as_ref().and_then(|vmo| vmo.duplicate_handle(rights).ok())
    }

    /// This produces a copy-on-write VMO with a generation count marked as
    /// VMO_FROZEN. The resulting VMO is read-only.
    ///
    /// Failure
    /// This function returns `None` for failure. That can happen for
    /// a few reasons.
    ///   1) It is a semantic error to freeze a VMO while an atomic transaction
    ///      is in progress, because that transaction is supposed to be atomic.
    ///   2) VMO errors. This can include running out of space or debug assertions.
    ///
    /// Note: the generation count for the original VMO is updated immediately. Since
    /// the new VMO is page-by-page copy-on-write, at least the first page of the
    /// VMO will immediately do a true copy. The practical implications of this
    /// depend on implementation details like how large a VMO is versus page size.
    pub fn frozen_vmo_copy(&self) -> Result<zx::Vmo, Error> {
        if let Some(state) = self.state() {
            let mut guard = state.try_lock()?;
            guard.frozen_vmo_copy()
        } else if let Some(vmo) = self.storage.as_ref() {
            if is_vmo_frozen(vmo) {
                // No-op Inspector was created with a frozen VMO, duplicate it.
                vmo.duplicate_handle(
                    // TODO(https://github.com/rust-lang/rust/issues/143802): Make this a const
                    // and share with duplicate_vmo().
                    zx::Rights::BASIC
                        | zx::Rights::READ
                        | zx::Rights::MAP
                        | zx::Rights::GET_PROPERTY,
                )
                .map_err(Error::DuplicateVmoFailed)
            } else {
                // No-op Inspector was created with a non-frozen VMO; we can't
                // freeze it since we don't have write rights to the VMO.
                Err(Error::VmoNotFrozen)
            }
        } else {
            Err(Error::VmoMissing)
        }
    }

    /// Returns a VMO holding a copy of the data in this inspector.
    ///
    /// The copied VMO will be read-only.
    pub fn copy_vmo(&self) -> Option<zx::Vmo> {
        self.copy_vmo_data().and_then(|data| {
            if let Ok(vmo) = zx::Vmo::create(data.len() as u64) {
                vmo.write(&data, 0).ok().map(|_| vmo)
            } else {
                None
            }
        })
    }

    pub(crate) fn get_storage_handle(&self) -> Option<Arc<zx::Vmo>> {
        // We can'just share a reference to the underlying vec<u8> storage, so we copy the data
        self.storage.clone()
    }

    #[cfg(test)]
    pub(crate) fn is_frozen(&self) -> bool {
        if let Some(vmo) = self.storage.as_ref() {
            return is_vmo_frozen(vmo);
        }
        false
    }
}

#[cfg(target_os = "fuchsia")]
fn is_vmo_frozen(vmo: impl AsRef<zx::Vmo>) -> bool {
    let mut buffer: [u8; 32] = [0; 32];
    if vmo.as_ref().read(&mut buffer, 0).is_ok() {
        let block = buffer.block_at_unchecked::<Header>(inspect_format::BlockIndex::EMPTY);
        return block.generation_count() == inspect_format::constants::VMO_FROZEN;
    }
    false
}

#[cfg(not(target_os = "fuchsia"))]
impl Inspector {
    pub(crate) fn duplicate_vmo(&self) -> Option<<Container as BlockContainer>::Data> {
        // We don't support getting a duplicate handle to the data on the host so we lock and copy
        // the udnerlying data.
        self.copy_vmo_data()
    }

    pub(crate) fn get_storage_handle(&self) -> Option<Vec<u8>> {
        // We can'just share a reference to the underlying vec<u8> storage, so we copy the data
        self.copy_vmo_data()
    }
}

impl Default for Inspector {
    fn default() -> Self {
        Inspector::new(InspectorConfig::default())
    }
}

impl Inspector {
    /// Initializes a new Inspect VMO object with the
    /// [`default maximum size`][constants::DEFAULT_VMO_SIZE_BYTES].
    pub fn new(conf: InspectorConfig) -> Self {
        conf.build()
    }

    /// Returns a copy of the bytes stored in the VMO for this inspector.
    ///
    /// The output will be truncated to only those bytes that are needed to accurately read the
    /// stored data.
    pub fn copy_vmo_data(&self) -> Option<Vec<u8>> {
        self.root_node.inner.inner_ref().and_then(|inner_ref| inner_ref.state.copy_vmo_bytes())
    }

    pub fn max_size(&self) -> Option<usize> {
        self.state()?.try_lock().ok().map(|state| state.stats().maximum_size)
    }

    /// True if the Inspector was created successfully (it's not No-Op)
    pub fn is_valid(&self) -> bool {
        // It is only necessary to check the root_node, because:
        //   1) If the Inspector was created as a no-op, the root node is not valid.
        //   2) If the creation of the Inspector failed, then the root_node is invalid. This
        //      is because `Inspector::new_root` returns the VMO and root node as a pair.
        self.root_node.is_valid()
    }

    /// Returns the root node of the inspect hierarchy.
    pub fn root(&self) -> &Node {
        &self.root_node
    }

    /// Takes a function to execute as under a single lock of the Inspect VMO. This function
    /// receives a reference to the root of the inspect hierarchy.
    pub fn atomic_update<F, R>(&self, update_fn: F) -> R
    where
        F: FnOnce(&Node) -> R,
    {
        self.root().atomic_update(update_fn)
    }

    pub(crate) fn state(&self) -> Option<State> {
        self.root().inner.inner_ref().map(|inner_ref| inner_ref.state.clone())
    }
}

/// Classic builder pattern object for constructing an `Inspector`.
pub struct InspectorConfig {
    is_no_op: bool,
    size: usize,
    storage: Option<Arc<<Container as BlockContainer>::ShareableData>>,
}

impl Default for InspectorConfig {
    /// A default Inspector:
    ///     * Fully functional
    ///     * Size: `constants::DEFAULT_VMO_SIZE_BYTES`
    ///
    /// Because the default is so cheap to construct, there is
    /// no "empty" `InspectorConfig`.
    fn default() -> Self {
        Self { is_no_op: false, size: constants::DEFAULT_VMO_SIZE_BYTES, storage: None }
    }
}

impl InspectorConfig {
    /// A read-only Inspector.
    pub fn no_op(mut self) -> Self {
        self.is_no_op = true;
        self
    }

    /// Size of the VMO.
    pub fn size(mut self, max_size: usize) -> Self {
        self.size = max_size;
        self
    }

    fn create_no_op(self) -> Inspector {
        Inspector { storage: self.storage, root_node: Arc::new(Node::new_no_op()) }
    }

    fn adjusted_buffer_size(max_size: usize) -> usize {
        let mut size = max(constants::MINIMUM_VMO_SIZE_BYTES, max_size);
        // If the size is not a multiple of 4096, round up.
        if !size.is_multiple_of(constants::MINIMUM_VMO_SIZE_BYTES) {
            size =
                (1 + size / constants::MINIMUM_VMO_SIZE_BYTES) * constants::MINIMUM_VMO_SIZE_BYTES;
        }

        size
    }
}

#[cfg(target_os = "fuchsia")]
impl InspectorConfig {
    /// An Inspector with a readable VMO.
    /// Implicitly no-op.
    pub fn vmo(mut self, vmo: zx::Vmo) -> Self {
        self.storage = Some(Arc::new(vmo));
        self.no_op()
    }

    fn build(self) -> Inspector {
        if self.is_no_op {
            return self.create_no_op();
        }

        match Self::new_root(self.size) {
            Ok((storage, root_node)) => {
                Inspector { storage: Some(storage), root_node: Arc::new(root_node) }
            }
            Err(e) => {
                error!("Failed to create root node. Error: {:?}", e);
                self.create_no_op()
            }
        }
    }

    /// Allocates a new VMO and initializes it.
    fn new_root(
        max_size: usize,
    ) -> Result<(Arc<<Container as BlockContainer>::ShareableData>, Node), Error> {
        let size = Self::adjusted_buffer_size(max_size);
        let (container, vmo) = Container::read_and_write(size).map_err(Error::AllocateVmo)?;
        let name = zx::Name::new("InspectHeap").unwrap();
        vmo.set_name(&name).map_err(Error::AllocateVmo)?;
        let vmo = Arc::new(vmo);
        let heap = Heap::new(container).map_err(|e| Error::CreateHeap(Box::new(e)))?;
        let state =
            State::create(heap, vmo.clone()).map_err(|e| Error::CreateState(Box::new(e)))?;
        Ok((vmo, Node::new_root(state)))
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl InspectorConfig {
    fn build(self) -> Inspector {
        if self.is_no_op {
            return self.create_no_op();
        }

        match Self::new_root(self.size) {
            Ok((root_node, storage)) => {
                Inspector { storage: Some(storage), root_node: Arc::new(root_node) }
            }
            Err(e) => {
                error!("Failed to create root node. Error: {:?}", e);
                self.create_no_op()
            }
        }
    }

    fn new_root(
        max_size: usize,
    ) -> Result<(Node, Arc<<Container as BlockContainer>::ShareableData>), Error> {
        let size = Self::adjusted_buffer_size(max_size);
        let (container, storage) = Container::read_and_write(size).unwrap();
        let heap = Heap::new(container).map_err(|e| Error::CreateHeap(Box::new(e)))?;
        let state =
            State::create(heap, Arc::new(storage)).map_err(|e| Error::CreateState(Box::new(e)))?;
        Ok((Node::new_root(state), Arc::new(storage)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_update_is_atomic;

    #[fuchsia::test]
    fn inspector_new() {
        let test_object = Inspector::default();
        assert_eq!(test_object.max_size().unwrap(), constants::DEFAULT_VMO_SIZE_BYTES);
    }

    #[fuchsia::test]
    fn inspector_copy_data() {
        let test_object = Inspector::default();

        assert_eq!(test_object.max_size().unwrap(), constants::DEFAULT_VMO_SIZE_BYTES);

        // The copy will be a single page, since that is all that is used.
        assert_eq!(test_object.copy_vmo_data().unwrap().len(), 4096);
    }

    #[fuchsia::test]
    fn no_op() {
        let inspector = Inspector::new(InspectorConfig::default().size(4096));
        // Make the VMO full.
        let nodes = (0..84)
            .map(|i| inspector.root().create_child(format!("test-{i}")))
            .collect::<Vec<Node>>();

        assert!(nodes.iter().all(|node| node.is_valid()));
        let no_op_node = inspector.root().create_child("no-op-child");
        assert!(!no_op_node.is_valid());
    }

    #[fuchsia::test]
    fn inspector_new_with_size() {
        let test_object = Inspector::new(InspectorConfig::default().size(8192));
        assert_eq!(test_object.max_size().unwrap(), 8192);

        // If size is not a multiple of 4096, it'll be rounded up.
        let test_object = Inspector::new(InspectorConfig::default().size(10000));
        assert_eq!(test_object.max_size().unwrap(), 12288);

        // If size is less than the minimum size, the minimum will be set.
        let test_object = Inspector::new(InspectorConfig::default().size(2000));
        assert_eq!(test_object.max_size().unwrap(), 4096);
    }

    #[fuchsia::test]
    async fn atomic_update() {
        let insp = Inspector::default();
        assert_update_is_atomic!(insp, |n| {
            n.record_int("", 1);
            n.record_int("", 2);
            n.record_uint("", 3);
            n.record_string("", "abcd");
        });
    }
}

// These tests exercise Fuchsia-specific APIs for Inspector.
#[cfg(all(test, target_os = "fuchsia"))]
mod fuchsia_tests {
    use super::*;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn inspector_duplicate_vmo() {
        let test_object = Inspector::default();
        assert_eq!(
            test_object.storage.as_ref().unwrap().get_size().unwrap(),
            constants::DEFAULT_VMO_SIZE_BYTES as u64
        );
        assert_eq!(
            test_object.duplicate_vmo().unwrap().get_size().unwrap(),
            constants::DEFAULT_VMO_SIZE_BYTES as u64
        );
    }

    #[fuchsia::test]
    fn inspector_new_root() {
        // Note, the small size we request should be rounded up to a full 4kB page.
        let (vmo, root_node) = InspectorConfig::new_root(100).unwrap();
        assert_eq!(vmo.get_size().unwrap(), 4096);
        let inner = root_node.inner.inner_ref().unwrap();
        assert_eq!(*inner.block_index, 0);
        assert_eq!("InspectHeap", vmo.get_name().expect("Has name"));
    }

    #[fuchsia::test]
    fn freeze_vmo_works() {
        let inspector = Inspector::default();
        let initial =
            inspector.state().unwrap().with_current_header(|header| header.generation_count());
        let vmo = inspector.frozen_vmo_copy();

        assert!(!inspector.is_frozen());

        let mut buffer: [u8; 32] = [0; 32];
        inspector.storage.as_ref().unwrap().read(&mut buffer, 0).unwrap();
        let block = buffer.block_at_unchecked::<Header>(inspect_format::BlockIndex::EMPTY);
        let gen_count = block.generation_count();

        assert_eq!(initial + 2, gen_count);
        assert!(gen_count.is_multiple_of(2));

        let frozen_insp = Inspector::new(InspectorConfig::default().no_op().vmo(vmo.unwrap()));
        assert!(frozen_insp.is_frozen());
    }

    #[fuchsia::test]
    fn no_op_inspector_with_frozen_vmo_can_be_frozen() {
        const VMO_SIZE: u64 = 4096;

        let original_inspector = Inspector::new(InspectorConfig::default().size(VMO_SIZE as usize));
        let frozen_vmo = original_inspector.frozen_vmo_copy().unwrap();
        let inspector = Inspector::new(InspectorConfig::default().no_op().vmo(frozen_vmo));

        assert_matches!(inspector.state(), None);
        assert_matches!(inspector.frozen_vmo_copy(), Ok(copy) => {
            assert_eq!(copy.get_size().unwrap(), VMO_SIZE);
        });
    }

    #[fuchsia::test]
    fn no_op_inspector_with_non_frozen_vmo_cannot_be_frozen() {
        let inspect_vmo = zx::Vmo::create(4096).unwrap();
        let inspector = Inspector::new(InspectorConfig::default().no_op().vmo(inspect_vmo));

        assert_matches!(inspector.state(), None);
        assert_matches!(inspector.frozen_vmo_copy(), Err(Error::VmoNotFrozen));
    }

    #[fuchsia::test]
    fn transactions_block_freezing() {
        let inspector = Inspector::default();
        inspector.atomic_update(|_| assert!(inspector.frozen_vmo_copy().is_err()));
    }

    #[fuchsia::test]
    fn transactions_block_copying() {
        let inspector = Inspector::default();
        inspector.atomic_update(|_| assert!(inspector.copy_vmo().is_none()));
        inspector.atomic_update(|_| assert!(inspector.copy_vmo_data().is_none()));
    }

    #[fuchsia::test]
    fn inspector_new_with_size() {
        let test_object = Inspector::new(InspectorConfig::default().size(8192));
        assert_eq!(test_object.max_size().unwrap(), 8192);

        assert_eq!(
            "InspectHeap",
            test_object.storage.as_ref().unwrap().get_name().expect("Has name")
        );

        // If size is not a multiple of 4096, it'll be rounded up.
        let test_object = Inspector::new(InspectorConfig::default().size(10000));
        assert_eq!(test_object.max_size().unwrap(), 12288);

        // If size is less than the minimum size, the minimum will be set.
        let test_object = Inspector::new(InspectorConfig::default().size(2000));
        assert_eq!(test_object.max_size().unwrap(), 4096);
    }
}
