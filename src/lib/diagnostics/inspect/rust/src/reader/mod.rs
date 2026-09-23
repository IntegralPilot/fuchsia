// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # Reading inspect data
//!
//! Provides an API for reading inspect data from a given [`Inspector`][Inspector], VMO, byte
//! vectors, etc.
//!
//! ## Concepts
//!
//! ### Diagnostics hierarchy
//!
//! Represents the inspect VMO as a regular tree of data. The API ensures that this structure
//! always contains the lazy values loaded as well.
//!
//! ### Partial node hierarchy
//!
//! Represents the inspect VMO as a regular tree of data, but unlike the Diagnostics Hierarchy, it
//! won't contain the lazy values loaded. An API is provided for converting this to a diagnostics
//! hierarchy ([`Into<DiagnosticsHierarchy>`](impl-Into<DiagnosticsHierarchy<String>>)), but keep
//! in mind that the resulting diagnostics hierarchy won't contain any of the lazy values.
//!
//! ## Example usage
//!
//! ```rust
//! use fuchsia_inspect::{Inspector, reader};
//!
//! let inspector = Inspector::default();
//! // ...
//! let hierarchy = reader::read(&inspector)?;
//! ```

use crate::reader::snapshot::{MakePrimitiveProperty, ScannedBlock, Snapshot};
use diagnostics_hierarchy::*;
use inspect_format::{
    Array, BlockIndex, BlockType, Bool, Buffer, Double, Int, Link, Node, Uint, Unknown,
    ValueBlockKind,
};
use maplit::btreemap;
use std::borrow::Cow;
use std::collections::BTreeMap;

pub use crate::reader::error::ReaderError;
pub use crate::reader::readable_tree::ReadableTree;
pub use crate::reader::tree_reader::{read, read_with_timeout};
pub use diagnostics_hierarchy::{ArrayContent, ArrayFormat, DiagnosticsHierarchy, Property};
pub use inspect_format::LinkNodeDisposition;

mod error;
mod readable_tree;
pub mod snapshot;
mod tree_reader;

/// A partial node hierarchy represents a node in an inspect tree without
/// the linked (lazy) nodes expanded.
/// Usually a client would prefer to use a `DiagnosticsHierarchy` to get the full
/// inspect tree.
#[derive(Clone, Debug, PartialEq)]
pub struct PartialNodeHierarchy {
    /// The name of this node.
    pub(crate) name: String,

    /// The properties for the node.
    pub(crate) properties: Vec<Property>,

    /// The children of this node.
    pub(crate) children: Vec<PartialNodeHierarchy>,

    /// Links this node hierarchy haven't expanded yet.
    pub(crate) links: Vec<LinkValue>,
}

/// A lazy node in a hierarchy.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct LinkValue {
    /// The name of the link.
    pub name: String,

    /// The content of the link.
    pub content: String,

    /// The disposition of the link in the hierarchy when evaluated.
    pub disposition: LinkNodeDisposition,
}

impl PartialNodeHierarchy {
    /// Creates an `PartialNodeHierarchy` with the given `name`, `properties` and `children`
    pub fn new(
        name: impl Into<String>,
        properties: Vec<Property>,
        children: Vec<PartialNodeHierarchy>,
    ) -> Self {
        Self { name: name.into(), properties, children, links: vec![] }
    }

    /// Creates an empty `PartialNodeHierarchy`
    pub fn empty() -> Self {
        PartialNodeHierarchy::new("", vec![], vec![])
    }

    /// Whether the partial hierarchy is complete or not. A complete node hierarchy
    /// has all the links loaded into it.
    pub fn is_complete(&self) -> bool {
        self.links.is_empty()
    }
}

/// Transforms the partial hierarchy into a `DiagnosticsHierarchy`. If the node hierarchy had
/// unexpanded links, those will appear as missing values.
impl From<PartialNodeHierarchy> for DiagnosticsHierarchy {
    fn from(partial: PartialNodeHierarchy) -> DiagnosticsHierarchy {
        DiagnosticsHierarchy {
            name: partial.name,
            children: partial.children.into_iter().map(|child| child.into()).collect(),
            properties: partial.properties,
            missing: partial
                .links
                .into_iter()
                .map(|link_value| MissingValue {
                    reason: MissingValueReason::LinkNeverExpanded,
                    name: link_value.name,
                })
                .collect(),
        }
    }
}

impl DiagnosticsHierarchyGetter<String> for PartialNodeHierarchy {
    async fn get_diagnostics_hierarchy<'a>(&'a self) -> Cow<'_, DiagnosticsHierarchy>
    where
        String: 'a,
    {
        let hierarchy: DiagnosticsHierarchy = self.clone().into();
        if !hierarchy.missing.is_empty() {
            panic!(
                "Missing links: {:?}",
                hierarchy
                    .missing
                    .iter()
                    .map(|missing| {
                        format!("(name:{:?}, reason:{:?})", missing.name, missing.reason)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Cow::Owned(hierarchy)
    }
}

impl TryFrom<Snapshot> for PartialNodeHierarchy {
    type Error = ReaderError;

    fn try_from(snapshot: Snapshot) -> Result<Self, Self::Error> {
        read_snapshot(&snapshot)
    }
}

#[cfg(target_os = "fuchsia")]
impl TryFrom<&zx::Vmo> for PartialNodeHierarchy {
    type Error = ReaderError;

    fn try_from(vmo: &zx::Vmo) -> Result<Self, Self::Error> {
        let snapshot = Snapshot::try_from(vmo)?;
        read_snapshot(&snapshot)
    }
}

impl TryFrom<Vec<u8>> for PartialNodeHierarchy {
    type Error = ReaderError;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let snapshot = Snapshot::try_from(bytes)?;
        read_snapshot(&snapshot)
    }
}

/// Read the blocks in the snapshot as a node hierarchy.
fn read_snapshot(snapshot: &Snapshot) -> Result<PartialNodeHierarchy, ReaderError> {
    let result = scan_blocks(snapshot)?;
    result.reduce()
}

fn scan_blocks(snapshot: &Snapshot) -> Result<ScanResult<'_>, ReaderError> {
    let mut result = ScanResult::new(snapshot);
    for block in snapshot.scan() {
        if block.index() == BlockIndex::ROOT && block.block_type() != Some(BlockType::Header) {
            return Err(ReaderError::MissingHeader);
        }
        match block.block_type().ok_or(ReaderError::InvalidVmo)? {
            BlockType::NodeValue => {
                result.parse_node(block.cast_unchecked::<Node>())?;
            }
            BlockType::IntValue => {
                result.parse_primitive_property(block.cast_unchecked::<Int>())?;
            }
            BlockType::UintValue => {
                result.parse_primitive_property(block.cast_unchecked::<Uint>())?;
            }
            BlockType::DoubleValue => {
                result.parse_primitive_property(block.cast_unchecked::<Double>())?;
            }
            BlockType::BoolValue => {
                result.parse_primitive_property(block.cast_unchecked::<Bool>())?;
            }
            BlockType::ArrayValue => {
                result.parse_array_property(block.cast_unchecked::<Array<Unknown>>())?;
            }
            BlockType::BufferValue => {
                result.parse_property(block.cast_unchecked::<Buffer>())?;
            }
            BlockType::LinkValue => {
                result.parse_link(block.cast_unchecked::<Link>())?;
            }
            BlockType::Free
            | BlockType::Reserved
            | BlockType::Header
            | BlockType::Extent
            | BlockType::Name
            | BlockType::Tombstone
            | BlockType::StringReference => {}
        }
    }
    Ok(result)
}

/// Result of scanning a snapshot before aggregating hierarchies.
struct ScanResult<'a> {
    /// All the nodes found while scanning the snapshot.
    /// Scanned nodes NodeHierarchies won't have their children filled.
    parsed_nodes: BTreeMap<BlockIndex, ScannedNode>,

    /// A snapshot of the Inspect VMO tree.
    snapshot: &'a Snapshot,
}

/// A scanned node in the Inspect VMO tree.
#[derive(Debug)]
struct ScannedNode {
    /// The node hierarchy with properties and children nodes filled.
    partial_hierarchy: PartialNodeHierarchy,

    /// The number of children nodes this node has.
    child_nodes_count: usize,

    /// The index of the parent node of this node.
    parent_index: BlockIndex,

    /// True only if this node was intialized. Uninitialized nodes will be ignored.
    initialized: bool,
}

impl ScannedNode {
    fn new() -> Self {
        ScannedNode {
            partial_hierarchy: PartialNodeHierarchy::empty(),
            child_nodes_count: 0,
            parent_index: BlockIndex::EMPTY,
            initialized: false,
        }
    }

    /// Sets the name and parent index of the node.
    fn initialize(&mut self, name: String, parent_index: BlockIndex) {
        self.partial_hierarchy.name = name;
        self.parent_index = parent_index;
        self.initialized = true;
    }

    /// A scanned node is considered complete if the number of children in the
    /// hierarchy is the same as the number of children counted while scanning.
    fn is_complete(&self) -> bool {
        self.partial_hierarchy.children.len() == self.child_nodes_count
    }

    /// A scanned node is considered initialized if a NodeValue was parsed for it.
    fn is_initialized(&self) -> bool {
        self.initialized
    }
}

macro_rules! get_or_create_scanned_node {
    ($map:expr, $key:expr) => {
        $map.entry($key).or_insert(ScannedNode::new())
    };
}

impl<'a> ScanResult<'a> {
    fn new(snapshot: &'a Snapshot) -> Self {
        let mut root_node = ScannedNode::new();
        root_node.initialize("root".to_string(), BlockIndex::ROOT);
        let parsed_nodes = btreemap!(
            BlockIndex::ROOT => root_node,
        );
        ScanResult { snapshot, parsed_nodes }
    }

    fn reduce(self) -> Result<PartialNodeHierarchy, ReaderError> {
        // Stack of nodes that have been found that are complete.
        let mut complete_nodes = Vec::<ScannedNode>::new();

        // Maps a block index to the node there. These nodes are still not
        // complete.
        let mut pending_nodes = BTreeMap::<BlockIndex, ScannedNode>::new();

        let mut uninitialized_nodes = std::collections::BTreeSet::new();

        // Split the parsed_nodes into complete nodes and pending nodes.
        for (index, scanned_node) in self.parsed_nodes.into_iter() {
            if !scanned_node.is_initialized() {
                // Skip all nodes that were not initialized.
                uninitialized_nodes.insert(index);
                continue;
            }
            if scanned_node.is_complete() {
                if index == BlockIndex::ROOT {
                    return Ok(scanned_node.partial_hierarchy);
                }
                complete_nodes.push(scanned_node);
            } else {
                pending_nodes.insert(index, scanned_node);
            }
        }

        // Build a valid hierarchy by attaching completed nodes to their parent.
        // Once the parent is complete, it's added to the stack and we recurse
        // until the root is found (parent index = 0).
        while let Some(scanned_node) = complete_nodes.pop() {
            if uninitialized_nodes.contains(&scanned_node.parent_index) {
                // Skip children of initialized nodes. These nodes were implicitly unlinked due to
                // tombstoning.
                continue;
            }
            {
                // Add the current node to the parent hierarchy.
                let parent_node = pending_nodes
                    .get_mut(&scanned_node.parent_index)
                    .ok_or(ReaderError::ParentIndexNotFound(scanned_node.parent_index))?;
                parent_node.partial_hierarchy.children.push(scanned_node.partial_hierarchy);
            }
            if pending_nodes
                .get(&scanned_node.parent_index)
                .ok_or(ReaderError::ParentIndexNotFound(scanned_node.parent_index))?
                .is_complete()
            {
                // Safety: if pending_nodes did not contain scanned_node.parent_index,
                // we would've returned above with ParentIndexNotFound
                let parent_node = pending_nodes.remove(&scanned_node.parent_index).unwrap();
                if scanned_node.parent_index == BlockIndex::ROOT {
                    return Ok(parent_node.partial_hierarchy);
                }
                complete_nodes.push(parent_node);
            }
        }

        Err(ReaderError::MalformedTree)
    }

    pub fn get_name(&self, index: BlockIndex) -> Option<String> {
        self.snapshot.get_name(index)
    }

    fn parse_node(&mut self, block: ScannedBlock<'_, Node>) -> Result<(), ReaderError> {
        let name_index = block.name_index();
        let name = self.get_name(name_index).ok_or(ReaderError::ParseName(name_index))?;
        let parent_index = block.parent_index();
        get_or_create_scanned_node!(self.parsed_nodes, block.index())
            .initialize(name, parent_index);
        if parent_index != block.index() {
            get_or_create_scanned_node!(self.parsed_nodes, parent_index).child_nodes_count += 1;
        }
        Ok(())
    }

    fn push_property(
        &mut self,
        parent_index: BlockIndex,
        property: Property,
    ) -> Result<(), ReaderError> {
        let parent = get_or_create_scanned_node!(self.parsed_nodes, parent_index);
        parent.partial_hierarchy.properties.push(property);
        Ok(())
    }

    fn parse_primitive_property<'b, K>(
        &mut self,
        block: ScannedBlock<'b, K>,
    ) -> Result<(), ReaderError>
    where
        K: ValueBlockKind,
        ScannedBlock<'b, K>: MakePrimitiveProperty,
    {
        let parent_index = block.parent_index();
        let property = self.snapshot.parse_primitive_property(block)?;
        self.push_property(parent_index, property)?;
        Ok(())
    }

    fn parse_array_property(
        &mut self,
        block: ScannedBlock<'_, Array<Unknown>>,
    ) -> Result<(), ReaderError> {
        let parent_index = block.parent_index();
        let property = self.snapshot.parse_array_property(block)?;
        self.push_property(parent_index, property)?;
        Ok(())
    }

    fn parse_property(&mut self, block: ScannedBlock<'_, Buffer>) -> Result<(), ReaderError> {
        let parent_index = block.parent_index();
        let property = self.snapshot.parse_property(block)?;
        self.push_property(parent_index, property)?;
        Ok(())
    }

    fn parse_link(&mut self, block: ScannedBlock<'_, Link>) -> Result<(), ReaderError> {
        let parent_index = block.parent_index();
        let parent = get_or_create_scanned_node!(self.parsed_nodes, parent_index);
        parent.partial_hierarchy.links.push(self.snapshot.parse_link(block)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::private::InspectTypeInternal;
    use crate::writer::testing_utils::GetBlockExt;
    use crate::{ArrayProperty, HistogramProperty, Inspector};
    use anyhow::Error;
    use diagnostics_assertions::{assert_data_tree, assert_json_diff};
    use futures::prelude::*;
    use inspect_format::{BlockContainer, CopyBytes, PayloadFields, constants};

    #[fuchsia::test]
    async fn test_load_string_reference() {
        let inspector = Inspector::default();
        let root = inspector.root();

        let name_value = "abc";
        let longer_name_value = "abcdefg";

        let child = root.create_child(name_value);
        child.record_int(name_value, 5);

        root.record_bool(longer_name_value, false);

        let result = read(&inspector).await.unwrap();
        assert_json_diff!(result, root: {
            abc: {
                abc: 5i64,
            },
            abcdefg: false,
        });
    }

    #[fuchsia::test]
    async fn read_string_array() {
        let inspector = Inspector::default();
        let root = inspector.root();

        let zero = (0..3000).map(|_| '0').collect::<String>();
        let one = "1";
        let two = "two";
        let three = "three three three";
        let four = "fourth";

        let array = root.create_string_array("array", 5);
        array.set(0, zero.as_str());
        array.set(1, one);
        array.set(2, two);
        array.set(3, three);
        array.set(4, four);

        let result = read(&inspector).await.unwrap();
        assert_json_diff!(result, root: {
            "array": vec![
                zero,
                one.to_string(),
                two.to_string(),
                three.to_string(),
                four.to_string(),
            ],
        });
    }

    #[fuchsia::test]
    async fn read_unset_string_array() {
        let inspector = Inspector::default();
        let root = inspector.root();

        let zero = (0..3000).map(|_| '0').collect::<String>();
        let one = "1";
        let four = "fourth";

        let array = root.create_string_array("array", 5);
        array.set(0, zero.as_str());
        array.set(1, one);
        array.set(4, four);

        let result = read(&inspector).await.unwrap();
        assert_json_diff!(result, root: {
            "array": vec![zero, one.to_string(), "".into(), "".into(), four.to_string()],
        });
    }

    #[fuchsia::test]
    async fn read_vmo() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let _root_int = root.create_int("int-root", 3);
        let root_double_array = root.create_double_array("property-double-array", 5);
        let double_array_data = vec![-1.2, 2.3, 3.4, 4.5, -5.6];
        for (i, x) in double_array_data.iter().enumerate() {
            root_double_array.set(i, *x);
        }

        let child1 = root.create_child("child-1");
        let _child1_uint = child1.create_uint("property-uint", 10);
        let _child1_double = child1.create_double("property-double", -3.4);
        let _child1_bool = child1.create_bool("property-bool", true);

        let chars = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];
        let string_data = chars.iter().cycle().take(6000).collect::<String>();
        let _string_prop = child1.create_string("property-string", &string_data);

        let child1_int_array = child1.create_int_linear_histogram(
            "property-int-array",
            LinearHistogramParams { floor: 1, step_size: 2, buckets: 3 },
        );
        for x in [-1, 2, 3, 5, 8].iter() {
            child1_int_array.insert(*x);
        }

        let child2 = root.create_child("child-2");
        let _child2_double = child2.create_double("property-double", 5.8);
        let _child2_bool = child2.create_bool("property-bool", false);

        let child3 = child1.create_child("child-1-1");
        let _child3_int = child3.create_int("property-int", -9);
        let bytes_data = (0u8..=9u8).cycle().take(5000).collect::<Vec<u8>>();
        let _bytes_prop = child3.create_bytes("property-bytes", &bytes_data);

        let child3_uint_array = child3.create_uint_exponential_histogram(
            "property-uint-array",
            ExponentialHistogramParams {
                floor: 1,
                initial_step: 1,
                step_multiplier: 2,
                buckets: 5,
            },
        );
        for x in [1, 2, 3, 4].iter() {
            child3_uint_array.insert(*x);
        }

        let result = read(&inspector).await.unwrap();

        assert_data_tree!(result, root: {
            "int-root": 3i64,
            "property-double-array": double_array_data,
            "child-1": {
                "property-uint": 10u64,
                "property-double": -3.4,
                "property-bool": true,
                "property-string": string_data,
                "property-int-array": LinearHistogram {
                    floor: 1i64,
                    step: 2,
                    counts: vec![1, 1, 1, 1, 1],
                    indexes: None,
                    size: 5
                },
                "child-1-1": {
                    "property-int": -9i64,
                    "property-bytes": bytes_data,
                    "property-uint-array": ExponentialHistogram {
                        floor: 1u64,
                        initial_step: 1,
                        step_multiplier: 2,
                        counts: vec![1, 1, 2],
                        indexes: Some(vec![1, 2, 3]),
                        size: 7
                    },
                }
            },
            "child-2": {
                "property-double": 5.8,
                "property-bool": false,
            }
        })
    }

    #[fuchsia::test]
    async fn siblings_with_same_name() {
        let inspector = Inspector::default();

        let foo = "foo";

        inspector.root().record_int("foo", 0);
        inspector.root().record_int("foo", 1);
        inspector.root().record_int(foo, 2);
        inspector.root().record_int(foo, 3);

        let dh = read(&inspector).await.unwrap();
        assert_eq!(dh.properties.len(), 4);
        for (i, property) in dh.properties.iter().enumerate() {
            match property {
                Property::Int(n, v) => {
                    assert_eq!(n, "foo");
                    assert_eq!(*v, i as i64);
                }
                _ => panic!("We only record int properties"),
            }
        }
    }

    #[fuchsia::test]
    async fn tombstone_reads() {
        let inspector = Inspector::default();
        let node1 = inspector.root().create_child("child1");
        let node2 = node1.create_child("child2");
        let node3 = node2.create_child("child3");
        let prop1 = node1.create_string("val", "test");
        let prop2 = node2.create_string("val", "test");
        let prop3 = node3.create_string("val", "test");

        assert_json_diff!(inspector,
            root: {
                child1: {
                    val: "test",
                    child2: {
                        val: "test",
                        child3: {
                            val: "test",
                        }
                    }
                }
            }
        );

        std::mem::drop(node3);
        assert_json_diff!(inspector,
            root: {
                child1: {
                    val: "test",
                    child2: {
                        val: "test",
                    }
                }
            }
        );

        std::mem::drop(node2);
        assert_json_diff!(inspector,
            root: {
                child1: {
                    val: "test",
                }
            }
        );

        // Recreate the nodes. Ensure that the old properties are not picked up.
        let node2 = node1.create_child("child2");
        let _node3 = node2.create_child("child3");
        assert_json_diff!(inspector,
            root: {
                child1: {
                    val: "test",
                    child2: {
                        child3: {}
                    }
                }
            }
        );

        // Delete out of order, leaving 3 dangling.
        std::mem::drop(node2);
        assert_json_diff!(inspector,
            root: {
                child1: {
                    val: "test",
                }
            }
        );

        std::mem::drop(node1);
        assert_json_diff!(inspector,
            root: {
            }
        );

        std::mem::drop(prop3);
        assert_json_diff!(inspector,
            root: {
            }
        );

        std::mem::drop(prop2);
        assert_json_diff!(inspector,
            root: {
            }
        );

        std::mem::drop(prop1);
        assert_json_diff!(inspector,
            root: {
            }
        );
    }

    #[fuchsia::test]
    async fn from_invalid_utf8_string() {
        // Creates a perfectly normal Inspector with a perfectly normal string
        // property with a perfectly normal value.
        let inspector = Inspector::default();
        let root = inspector.root();
        let prop = root.create_string("property", "hello world");

        // Now we will excavate the bytes that comprise the string property, then mess with them on
        // purpose to produce an invalid UTF8 string in the property.
        let vmo = inspector.vmo().await.unwrap();
        let snapshot = Snapshot::try_from(&vmo).expect("getting snapshot");
        let block = snapshot
            .get_block(prop.block_index().unwrap())
            .expect("getting block")
            .cast::<Buffer>()
            .unwrap();

        // The first byte of the actual property string is at this byte offset in the VMO.
        let byte_offset = constants::MIN_ORDER_SIZE * (*block.extent_index() as usize)
            + constants::HEADER_SIZE_BYTES
            + constants::STRING_REFERENCE_TOTAL_LENGTH_BYTES;

        // Get the raw VMO bytes to mess with.
        let vmo_size = BlockContainer::len(&vmo);
        let mut buf = vec![0u8; vmo_size];
        vmo.copy_bytes(&mut buf[..]);

        // Mess up the first byte of the string property value such that the byte is an invalid
        // UTF8 character.  Then build a new node hierarchy based off those bytes, see if invalid
        // string is converted into a valid UTF8 string with some information lost.
        buf[byte_offset] = 0xFE;
        let hierarchy: DiagnosticsHierarchy = PartialNodeHierarchy::try_from(Snapshot::build(&buf))
            .expect("creating node hierarchy")
            .into();

        assert_json_diff!(hierarchy, root: {
            property: "\u{FFFD}ello world",
        });
    }

    #[fuchsia::test]
    async fn test_invalid_array_slots() -> Result<(), Error> {
        let inspector = Inspector::default();
        let root = inspector.root();
        let array = root.create_int_array("int-array", 3);

        // Mess up with the block slots by setting them to a too big number.
        array.get_block_mut::<_, Array<Int>>(|array_block| {
            PayloadFields::set_array_slots_count(array_block, 255);
        });

        let vmo = inspector.vmo().await.unwrap();
        let vmo_size = BlockContainer::len(&vmo);

        let mut buf = vec![0u8; vmo_size];
        vmo.copy_bytes(&mut buf[..]);

        assert!(PartialNodeHierarchy::try_from(Snapshot::build(&buf)).is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn lazy_nodes() -> Result<(), Error> {
        let inspector = Inspector::default();
        inspector.root().record_int("int", 3);
        let child = inspector.root().create_child("child");
        child.record_double("double", 1.5);
        inspector.root().record_lazy_child("lazy", || {
            async move {
                let inspector = Inspector::default();
                inspector.root().record_uint("uint", 5);
                inspector.root().record_lazy_values("nested-lazy-values", || {
                    async move {
                        let inspector = Inspector::default();
                        inspector.root().record_string("string", "test");
                        let child = inspector.root().create_child("nested-lazy-child");
                        let array = child.create_int_array("array", 3);
                        array.set(0, 1);
                        child.record(array);
                        inspector.root().record(child);
                        Ok(inspector)
                    }
                    .boxed()
                });
                Ok(inspector)
            }
            .boxed()
        });

        inspector.root().record_lazy_values("lazy-values", || {
            async move {
                let inspector = Inspector::default();
                let child = inspector.root().create_child("lazy-child-1");
                child.record_string("test", "testing");
                inspector.root().record(child);
                inspector.root().record_uint("some-uint", 3);
                inspector.root().record_lazy_values("nested-lazy-values", || {
                    async move {
                        let inspector = Inspector::default();
                        inspector.root().record_int("lazy-int", -3);
                        let child = inspector.root().create_child("one-more-child");
                        child.record_double("lazy-double", 4.3);
                        inspector.root().record(child);
                        Ok(inspector)
                    }
                    .boxed()
                });
                inspector.root().record_lazy_child("nested-lazy-child", || {
                    async move {
                        let inspector = Inspector::default();
                        // This will go out of scope and is not recorded, so it shouldn't appear.
                        let _double = inspector.root().create_double("double", -1.2);
                        Ok(inspector)
                    }
                    .boxed()
                });
                Ok(inspector)
            }
            .boxed()
        });

        let hierarchy = read(&inspector).await?;
        assert_json_diff!(hierarchy, root: {
            int: 3i64,
            child: {
                double: 1.5,
            },
            lazy: {
                uint: 5u64,
                string: "test",
                "nested-lazy-child": {
                    array: vec![1i64, 0, 0],
                }
            },
            "some-uint": 3u64,
            "lazy-child-1": {
                test: "testing",
            },
            "lazy-int": -3i64,
            "one-more-child": {
                "lazy-double": 4.3,
            },
            "nested-lazy-child": {
            }
        });

        Ok(())
    }

    #[fuchsia::test]
    async fn test_matching_with_inspector() {
        let inspector = Inspector::default();
        assert_json_diff!(inspector, root: {});
    }

    #[fuchsia::test]
    async fn test_matching_with_partial() {
        let propreties = vec![Property::String("sub".to_string(), "sub_value".to_string())];
        let partial = PartialNodeHierarchy::new("root", propreties, vec![]);
        assert_json_diff!(partial, root: {
            sub: "sub_value",
        });
    }

    #[fuchsia::test]
    #[should_panic]
    async fn test_missing_values_with_partial() {
        let mut partial = PartialNodeHierarchy::new("root", vec![], vec![]);
        partial.links = vec![LinkValue {
            name: "missing-link".to_string(),
            content: "missing-link-404".to_string(),
            disposition: LinkNodeDisposition::Child,
        }];
        assert_json_diff!(partial, root: {});
    }

    #[fuchsia::test]
    async fn test_matching_with_expression_as_key() {
        let properties = vec![Property::String("sub".to_string(), "sub_value".to_string())];
        let partial = PartialNodeHierarchy::new("root", properties, vec![]);
        let value = || "sub_value";
        let key = || "sub".to_string();
        assert_json_diff!(partial, root: {
            key() => value(),
        });
    }
}
