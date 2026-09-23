// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Data, LazyNode, Metrics, Node, Payload, Property, ROOT_NAME};
use crate::metrics::{BlockMetrics, BlockStatus};
use anyhow::{Context, Error, bail, format_err};
use fuchsia_inspect::reader as ireader;
use fuchsia_inspect::reader::snapshot::ScannedBlock;
use inspect_format::constants::MIN_ORDER_SIZE;
use inspect_format::*;
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use zx::Vmo;

// When reading from a VMO, the keys of the HashMaps are the indexes of the relevant
// blocks. Thus, they will never collide.
//
// Reading from a VMO is a complicated process.
// 1) Try to take a fuchsia_inspect::reader::snapshot::Snapshot of the VMO.
// 2) Iterate through it, pedantically examining all its blocks and loading
//   the relevant blocks into a ScannedObjects structure (which contains
//   ScannedNode, ScannedName, ScannedProperty, and ScannedExtent).
// 2.5) ScannedNodes may be added before they're scanned, since they need to
//   track their child nodes and properties. In this case, their "validated"
//   field will be false until they're actually scanned.
// 3) Starting from the "0" node, create Node and Property objects for all the
//   dependent children and properties (verifying that all dependent objects
//   exist (and are valid in the case of Nodes)). This is also when Extents are
//   combined into byte vectors, and in the case of String, verified to be valid UTF-8.
// 4) Add the Node and Property objects (each with its ID) into the "nodes" and
//   "properties" HashMaps of a new Data object. Note that these HashMaps do not
//   hold the hierarchical information; instead, each Node contains a HashSet of
//   the keys of its children and properties.

#[derive(Debug)]
pub struct Scanner {
    nodes: HashMap<BlockIndex, ScannedNode>,
    names: HashMap<BlockIndex, ScannedName>,
    properties: HashMap<BlockIndex, ScannedProperty>,
    extents: HashMap<BlockIndex, ScannedExtent>,
    final_dereferenced_strings: HashMap<BlockIndex, String>,
    final_nodes: HashMap<BlockIndex, Node>,
    final_properties: HashMap<BlockIndex, Property>,
    metrics: Metrics,
    child_trees: Option<HashMap<String, LazyNode>>,
}

impl TryFrom<&[u8]> for Scanner {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let scanner = Scanner::new(None);
        scanner.scan(ireader::snapshot::Snapshot::try_from(bytes.to_vec())?, bytes)
    }
}

impl TryFrom<&Vmo> for Scanner {
    type Error = Error;
    fn try_from(vmo: &Vmo) -> Result<Self, Self::Error> {
        let scanner = Scanner::new(None);
        scanner.scan(ireader::snapshot::Snapshot::try_from(vmo)?, &vmo_as_buffer(vmo)?)
    }
}

impl TryFrom<LazyNode> for Scanner {
    type Error = Error;

    fn try_from(mut vmo_tree: LazyNode) -> Result<Self, Self::Error> {
        let snapshot = ireader::snapshot::Snapshot::try_from(vmo_tree.vmo())?;
        let buffer = vmo_as_buffer(vmo_tree.vmo())?;
        let scanner = Scanner::new(vmo_tree.take_children());
        scanner.scan(snapshot, &buffer)
    }
}

fn vmo_as_buffer(vmo: &Vmo) -> Result<Vec<u8>, Error> {
    // NOTE: In any context except a controlled test, it's not safe to read the VMO manually -
    // the contents may differ or even be invalid (mid-update).
    let size = vmo.get_size()?;
    let mut buffer = vec![0u8; size as usize];
    vmo.read(&mut buffer[..], 0)?;
    Ok(buffer)
}

fn low_bits(number: u8, n_bits: usize) -> u8 {
    let n_bits = min(n_bits, 8);
    let mask = !(0xff_u16 << n_bits) as u8;
    number & mask
}

fn high_bits(number: u8, n_bits: usize) -> u8 {
    let n_bits = min(n_bits, 8);
    let mask = !(0xff_u16 >> n_bits) as u8;
    number & mask
}

const BITS_PER_BYTE: usize = 8;

/// Get size in bytes of a given |order|. Copied from private mod fuchsia-inspect/src/utils.rs
fn order_to_size(order: u8) -> usize {
    MIN_ORDER_SIZE << (order as usize)
}

// Checks if these bits (start...end) are 0. Restricts the range checked to the given block.
fn check_zero_bits(
    buffer: &[u8],
    block: &ScannedBlock<'_, Unknown>,
    start: usize,
    end: usize,
) -> Result<(), Error> {
    if end < start {
        return Err(format_err!("End must be >= start"));
    }
    let bits_in_block = order_to_size(block.order()) * BITS_PER_BYTE;
    if start > bits_in_block - 1 {
        return Ok(());
    }
    let end = min(end, bits_in_block - 1);
    let block_offset = usize::from(block.index()) * MIN_ORDER_SIZE;
    let low_byte = start / BITS_PER_BYTE;
    let high_byte = end / BITS_PER_BYTE;
    let bottom_bits = high_bits(buffer[low_byte + block_offset], 8 - (start % 8));
    let top_bits = low_bits(buffer[high_byte + block_offset], (end % 8) + 1);
    if low_byte == high_byte {
        match bottom_bits & top_bits {
            0 => return Ok(()),
            nonzero => bail!(
                "Bits {}...{} of block type {:?} at {} have nonzero value {}",
                start,
                end,
                block.block_type(),
                block.index(),
                nonzero
            ),
        }
    }
    if bottom_bits != 0 {
        bail!(
            "Non-zero value {} for bits {}.. of block type {:?} at {}",
            bottom_bits,
            start,
            block.block_type(),
            block.index()
        );
    }
    if top_bits != 0 {
        bail!(
            "Non-zero value {} for bits ..{} of block type {:?} at {}",
            top_bits,
            end,
            block.block_type(),
            block.index()
        );
    }
    for byte in low_byte + 1..high_byte {
        if buffer[byte + block_offset] != 0 {
            bail!(
                "Non-zero value {} for byte {} of block type {:?} at {}",
                buffer[byte],
                byte,
                block.block_type(),
                block.index()
            );
        }
    }
    Ok(())
}

impl Scanner {
    fn new(child_trees: Option<HashMap<String, LazyNode>>) -> Scanner {
        let mut ret = Scanner {
            nodes: HashMap::new(),
            names: HashMap::new(),
            properties: HashMap::new(),
            extents: HashMap::new(),
            final_dereferenced_strings: HashMap::new(),
            metrics: Metrics::new(),
            final_nodes: HashMap::new(),
            final_properties: HashMap::new(),
            child_trees,
        };
        // The ScannedNode at 0 is the "root" node. It exists to receive pointers to objects
        // whose parent is 0 while scanning the VMO.
        ret.nodes.insert(
            BlockIndex::ROOT,
            ScannedNode {
                validated: true,
                parent: BlockIndex::ROOT,
                name: BlockIndex::ROOT,
                children: HashSet::new(),
                properties: HashSet::new(),
                metrics: None,
            },
        );
        ret
    }

    fn scan(mut self, snapshot: ireader::snapshot::Snapshot, buffer: &[u8]) -> Result<Self, Error> {
        let mut link_blocks: Vec<ScannedBlock<'_, Unknown>> = Vec::new();
        let mut string_references: Vec<ScannedStringReference> = Vec::new();
        for block in snapshot.scan() {
            match block.block_type().ok_or_else(|| format_err!("invalid block type"))? {
                BlockType::Free => self.process_free(block)?,
                BlockType::Reserved => self.process_reserved(block)?,
                BlockType::Header => self.process_header(block)?,
                BlockType::NodeValue => self.process_node(block)?,
                BlockType::IntValue => self.process_property::<Int>(block, buffer)?,
                BlockType::UintValue => self.process_property::<Uint>(block, buffer)?,
                BlockType::DoubleValue => self.process_property::<Double>(block, buffer)?,
                BlockType::ArrayValue => self.process_property::<Array<Unknown>>(block, buffer)?,
                BlockType::BufferValue => self.process_property::<Buffer>(block, buffer)?,
                BlockType::BoolValue => self.process_property::<Bool>(block, buffer)?,
                BlockType::LinkValue => link_blocks.push(block),
                BlockType::Extent => self.process_extent(block, buffer)?,
                BlockType::Name => self.process_name(block, buffer)?,
                BlockType::Tombstone => self.process_tombstone(block)?,
                BlockType::StringReference => {
                    string_references.push(self.process_string_reference(block)?);
                }
            }
        }

        for block in string_references.drain(..) {
            let index = block.index;
            let dereferenced = self.expand_string_reference(block)?;
            self.final_dereferenced_strings.insert(index, dereferenced);
        }

        // We defer processing LINK blocks after because the population of the
        // ScannedPayload::Link depends on all NAME blocks having been read.
        for block in link_blocks.into_iter() {
            self.process_property::<Link>(block, buffer)?
        }

        let (mut new_nodes, mut new_properties) = self.make_valid_node_tree(BlockIndex::ROOT)?;
        for (node, id) in new_nodes.drain(..) {
            self.final_nodes.insert(id, node);
        }
        for (property, id) in new_properties.drain(..) {
            self.final_properties.insert(id, property);
        }

        self.record_unused_metrics();
        Ok(self)
    }

    pub fn data(self) -> Data {
        Data::build(self.final_nodes, self.final_properties)
    }

    pub fn metrics(self) -> Metrics {
        self.metrics
    }

    // ***** Utility functions
    fn record_unused_metrics(&mut self) {
        for (_, node) in self.nodes.drain() {
            if let Some(metrics) = node.metrics {
                self.metrics.record(&metrics, BlockStatus::NotUsed);
            }
        }
        for (_, name) in self.names.drain() {
            self.metrics.record(&name.metrics, BlockStatus::NotUsed);
        }
        for (_, property) in self.properties.drain() {
            self.metrics.record(&property.metrics, BlockStatus::NotUsed);
        }
        for (_, extent) in self.extents.drain() {
            self.metrics.record(&extent.metrics, BlockStatus::NotUsed);
        }
    }

    fn use_node(&mut self, node_id: BlockIndex) -> Result<ScannedNode, Error> {
        let mut node = self
            .nodes
            .remove(&node_id)
            .ok_or_else(|| format_err!("No node at index {}", node_id))?;
        match node.metrics {
            None => {
                if node_id != BlockIndex::ROOT {
                    return Err(format_err!("Invalid node (no metrics) at index {}", node_id));
                }
            }
            Some(metrics) => {
                // I actually want as_deref() but that's nightly-only.
                self.metrics.record(&metrics, BlockStatus::Used);
                node.metrics = Some(metrics); // Put it back after I borrow it.
            }
        }
        Ok(node)
    }

    fn use_property(&mut self, property_id: BlockIndex) -> Result<ScannedProperty, Error> {
        let property = self
            .properties
            .remove(&property_id)
            .ok_or_else(|| format_err!("No property at index {}", property_id))?;
        self.metrics.record(&property.metrics, BlockStatus::Used);
        Ok(property)
    }

    // Used to find the value of a name index. This index, `name_id`, may refer to either a
    // NAME block or a STRING_REFERENCE. If the block is a NAME, it will be removed.
    fn use_owned_name(&mut self, name_id: BlockIndex) -> Result<String, Error> {
        match self.names.remove(&name_id) {
            Some(name) => {
                self.metrics.record(&name.metrics, BlockStatus::Used);
                Ok(name.name)
            }
            None => match self.final_dereferenced_strings.get(&name_id) {
                Some(value) => {
                    // Once a string is de-referenced, it isn't part of the hierarchy,
                    // so we use metrics.process(block) when we process a STRING_REFERENCE.
                    Ok(value.clone())
                }
                None => Err(format_err!("No string at index {}", name_id)),
            },
        }
    }

    // Used to find the value of an index that points to a NAME or STRING_REFERENCE. In either
    // case, the value is not consumed.
    fn lookup_name_or_string_reference(&mut self, name_id: BlockIndex) -> Result<String, Error> {
        match self.names.get(&name_id) {
            Some(name) => {
                self.metrics.record(&name.metrics, BlockStatus::Used);
                Ok(name.name.clone())
            }
            None => match self.final_dereferenced_strings.get(&name_id) {
                Some(value) => {
                    // Once a string is de-referenced, it isn't part of the hierarchy,
                    // so we use metrics.process(block) when we process a STRING_REFERENCE.
                    Ok(value.clone())
                }
                None => Err(format_err!("No string at index {}", name_id)),
            },
        }
    }

    // ***** Functions which read fuchsia_inspect::format::block::Block (actual
    // ***** VMO blocks), validate them, turn them into Scanned* objects, and
    // ***** add the ones we care about to Self.

    // Some blocks' metrics can only be calculated in the context of a tree. Metrics aren't run
    // on those in the process_ functions, but rather while the tree is being built.

    // Note: process_ functions are only called from the scan() iterator on the
    // VMO's blocks, so indexes of the blocks themselves will never be duplicated; that's one
    // thing we don't have to verify.
    fn process_free(&mut self, block: ScannedBlock<'_, Unknown>) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42115894): Uncomment or delete this line depending on the resolution of https://fxbug.dev/42115938.
        // check_zero_bits(buffer, &block, 64, MAX_BLOCK_BITS)?;
        self.metrics.process(block)?;
        Ok(())
    }

    fn process_header(&mut self, block: ScannedBlock<'_, Unknown>) -> Result<(), Error> {
        self.metrics.process(block)?;
        Ok(())
    }

    fn process_tombstone(&mut self, block: ScannedBlock<'_, Unknown>) -> Result<(), Error> {
        self.metrics.process(block)?;
        Ok(())
    }

    fn process_reserved(&mut self, block: ScannedBlock<'_, Unknown>) -> Result<(), Error> {
        self.metrics.process(block)?;
        Ok(())
    }

    fn process_extent(
        &mut self,
        block: ScannedBlock<'_, Unknown>,
        buffer: &[u8],
    ) -> Result<(), Error> {
        check_zero_bits(buffer, &block, 40, 63)?;
        let extent = block.clone().cast::<Extent>().unwrap();
        let next = extent.next_extent();
        let data = extent.contents()?.to_vec();
        self.extents
            .insert(block.index(), ScannedExtent { next, data, metrics: Metrics::analyze(block)? });
        Ok(())
    }

    fn process_name(
        &mut self,
        block: ScannedBlock<'_, Unknown>,
        buffer: &[u8],
    ) -> Result<(), Error> {
        check_zero_bits(buffer, &block, 28, 63)?;
        let name_block = block.clone().cast::<Name>().unwrap();
        let name = name_block.contents()?.to_string();
        self.names.insert(block.index(), ScannedName { name, metrics: Metrics::analyze(block)? });
        Ok(())
    }

    fn process_node(&mut self, block: ScannedBlock<'_, Unknown>) -> Result<(), Error> {
        let node_block = block.clone().cast::<inspect_format::Node>().unwrap();
        let parent = node_block.parent_index();
        let id = node_block.index();
        let name = node_block.name_index();
        let mut node;
        let metrics = Some(Metrics::analyze(block)?);
        if let Some(placeholder) = self.nodes.remove(&id) {
            // We need to preserve the children and properties.
            node = placeholder;
            node.validated = true;
            node.parent = parent;
            node.name = name;
            node.metrics = metrics;
        } else {
            node = ScannedNode {
                validated: true,
                name,
                parent,
                children: HashSet::new(),
                properties: HashSet::new(),
                metrics,
            }
        }
        self.nodes.insert(id, node);
        self.add_to_parent(parent, id, |node| &mut node.children);
        Ok(())
    }

    fn add_to_parent<F: FnOnce(&mut ScannedNode) -> &mut HashSet<BlockIndex>>(
        &mut self,
        parent: BlockIndex,
        id: BlockIndex,
        get_the_hashset: F, // Gets children or properties
    ) {
        self.nodes.entry(parent).or_insert_with(|| ScannedNode {
            validated: false,
            name: BlockIndex::EMPTY,
            parent: BlockIndex::ROOT,
            children: HashSet::new(),
            properties: HashSet::new(),
            metrics: None,
        });
        if let Some(parent_node) = self.nodes.get_mut(&parent) {
            get_the_hashset(parent_node).insert(id);
        }
    }

    fn process_string_reference(
        &mut self,
        block: ScannedBlock<'_, Unknown>,
    ) -> Result<ScannedStringReference, Error> {
        let string_ref = block.clone().cast::<StringRef>().unwrap();
        let scanned = ScannedStringReference {
            index: block.index(),
            value: string_ref.inline_data()?.to_vec(),
            length: string_ref.total_length(),
            next_extent: string_ref.next_extent(),
        };

        self.metrics.process(block)?;
        Ok(scanned)
    }

    fn process_property<K>(
        &mut self,
        block: ScannedBlock<'_, Unknown>,
        buffer: &[u8],
    ) -> Result<(), Error>
    where
        K: ValueBlockKind + Clone,
        ScannedPayload: BuildScannedPayload<K>,
    {
        if block.block_type() == Some(BlockType::ArrayValue) {
            check_zero_bits(buffer, &block, 80, 127)?;
        }
        let property_block = block.clone().cast::<K>().unwrap();
        let id = property_block.index();
        let parent = property_block.parent_index();
        let name = property_block.name_index();
        let payload = ScannedPayload::build(property_block, self)?;
        let property = ScannedProperty { name, parent, payload, metrics: Metrics::analyze(block)? };
        self.properties.insert(id, property);
        self.add_to_parent(parent, id, |node| &mut node.properties);
        Ok(())
    }

    // ***** Functions which convert Scanned* objects into Node and Property objects.

    #[allow(clippy::type_complexity)]
    fn make_valid_node_tree(
        &mut self,
        id: BlockIndex,
    ) -> Result<(Vec<(Node, BlockIndex)>, Vec<(Property, BlockIndex)>), Error> {
        let scanned_node = self.use_node(id)?;
        if !scanned_node.validated {
            return Err(format_err!("No node at {}", id));
        }
        let mut nodes_in_tree = vec![];
        let mut properties_under = vec![];
        for node_id in scanned_node.children.iter() {
            let (mut nodes_of, mut properties_of) = self.make_valid_node_tree(*node_id)?;
            nodes_in_tree.append(&mut nodes_of);
            properties_under.append(&mut properties_of);
        }
        for property_id in scanned_node.properties.iter() {
            properties_under.push((self.make_valid_property(*property_id)?, *property_id));
        }
        let name = if id == BlockIndex::ROOT {
            ROOT_NAME.to_owned()
        } else {
            self.use_owned_name(scanned_node.name)?
        };
        let this_node = Node {
            name,
            parent: scanned_node.parent,
            children: scanned_node.children.clone(),
            properties: scanned_node.properties.clone(),
        };
        nodes_in_tree.push((this_node, id));
        Ok((nodes_in_tree, properties_under))
    }

    fn make_valid_property(&mut self, id: BlockIndex) -> Result<Property, Error> {
        let scanned_property = self.use_property(id)?;
        let name = self.use_owned_name(scanned_property.name)?;
        let payload = self.make_valid_payload(scanned_property.payload)?;
        Ok(Property { id, name, parent: scanned_property.parent, payload })
    }

    fn make_valid_payload(&mut self, payload: ScannedPayload) -> Result<Payload, Error> {
        Ok(match payload {
            ScannedPayload::Int(data) => Payload::Int(data),
            ScannedPayload::Uint(data) => Payload::Uint(data),
            ScannedPayload::Double(data) => Payload::Double(data),
            ScannedPayload::Bool(data) => Payload::Bool(data),
            ScannedPayload::IntArray(data, format) => Payload::IntArray(data, format),
            ScannedPayload::UintArray(data, format) => Payload::UintArray(data, format),
            ScannedPayload::DoubleArray(data, format) => Payload::DoubleArray(data, format),
            ScannedPayload::StringArray(indexes) => Payload::StringArray(
                indexes
                    .iter()
                    .map(|i| {
                        if *i == BlockIndex::EMPTY {
                            return "".into();
                        }
                        self.final_dereferenced_strings.get(i).unwrap().clone()
                    })
                    .collect(),
            ),
            ScannedPayload::Bytes { length, link } => {
                Payload::Bytes(self.make_valid_vector(length, link)?)
            }
            ScannedPayload::String { length, link } => {
                Payload::String(if let Some(length) = length {
                    std::str::from_utf8(&self.make_valid_vector(length, link)?)?.to_owned()
                } else {
                    self.final_dereferenced_strings.get(&link).unwrap().clone()
                })
            }
            ScannedPayload::Link { disposition, scanned_tree } => {
                Payload::Link { disposition, parsed_data: scanned_tree.data() }
            }
        })
    }

    fn expand_string_reference(
        &mut self,
        mut block: ScannedStringReference,
    ) -> Result<String, Error> {
        let length_of_inlined = block.value.len();
        if block.next_extent != BlockIndex::EMPTY {
            block.value.append(
                &mut self.make_valid_vector(block.length - length_of_inlined, block.next_extent)?,
            );
        }

        Ok(String::from_utf8(block.value)?)
    }

    fn make_valid_vector(&mut self, length: usize, link: BlockIndex) -> Result<Vec<u8>, Error> {
        let mut dest = vec![];
        let mut length_remaining = length;
        let mut next_link = link;
        while length_remaining > 0 {
            // This is effectively use_extent()
            let mut extent = self
                .extents
                .remove(&next_link)
                .ok_or_else(|| format_err!("No extent at {}", next_link))?;
            let copy_len = min(extent.data.len(), length_remaining);
            extent.metrics.set_data_bytes(copy_len);
            self.metrics.record(&extent.metrics, BlockStatus::Used);
            dest.extend_from_slice(&extent.data[..copy_len]);
            length_remaining -= copy_len;
            next_link = extent.next;
        }
        Ok(dest)
    }
}

#[derive(Debug)]
struct ScannedNode {
    // These may be created two ways: Either from being named as a parent, or
    // from being processed in the VMO. Those named but not yet processed will
    // have validated = false. Of course after a complete VMO scan,
    // everything descended from a root node must be validated.
    // validated refers to the binary contents of this block; it doesn't
    // guarantee that properties, descendents, name, etc. are valid.
    validated: bool,
    name: BlockIndex,
    parent: BlockIndex,
    children: HashSet<BlockIndex>,
    properties: HashSet<BlockIndex>,
    metrics: Option<BlockMetrics>,
}

#[derive(Debug)]
struct ScannedProperty {
    name: BlockIndex,
    parent: BlockIndex,
    payload: ScannedPayload,
    metrics: BlockMetrics,
}

#[derive(Debug)]
struct ScannedStringReference {
    index: BlockIndex,
    value: Vec<u8>,
    length: usize,
    next_extent: BlockIndex,
}

#[derive(Debug)]
struct ScannedName {
    name: String,
    metrics: BlockMetrics,
}

#[derive(Debug)]
struct ScannedExtent {
    next: BlockIndex,
    data: Vec<u8>,
    metrics: BlockMetrics,
}

#[derive(Debug)]
enum ScannedPayload {
    String {
        // length might be `None` if `link` points to a `StringReference`, because the
        // `StringReference` encodes its own length parameter
        length: Option<usize>,
        link: BlockIndex,
    },
    Bytes {
        length: usize,
        link: BlockIndex,
    },
    Int(i64),
    Uint(u64),
    Double(f64),
    Bool(bool),
    IntArray(Vec<i64>, ArrayFormat),
    UintArray(Vec<u64>, ArrayFormat),
    DoubleArray(Vec<f64>, ArrayFormat),
    StringArray(Vec<BlockIndex>),
    Link {
        disposition: LinkNodeDisposition,
        scanned_tree: Box<Scanner>,
    },
}

trait BuildScannedPayload<K> {
    fn build(block: ScannedBlock<'_, K>, _scanner: &mut Scanner) -> Result<ScannedPayload, Error>;
}

impl BuildScannedPayload<Int> for ScannedPayload {
    fn build(block: ScannedBlock<'_, Int>, _scanner: &mut Scanner) -> Result<Self, Error> {
        Ok(Self::Int(block.value()))
    }
}

impl BuildScannedPayload<Uint> for ScannedPayload {
    fn build(block: ScannedBlock<'_, Uint>, _scanner: &mut Scanner) -> Result<Self, Error> {
        Ok(Self::Uint(block.value()))
    }
}

impl BuildScannedPayload<Double> for ScannedPayload {
    fn build(block: ScannedBlock<'_, Double>, _scanner: &mut Scanner) -> Result<Self, Error> {
        Ok(Self::Double(block.value()))
    }
}

impl BuildScannedPayload<Bool> for ScannedPayload {
    fn build(block: ScannedBlock<'_, Bool>, _scanner: &mut Scanner) -> Result<Self, Error> {
        Ok(Self::Bool(block.value()))
    }
}

impl BuildScannedPayload<Buffer> for ScannedPayload {
    fn build(block: ScannedBlock<'_, Buffer>, _scanner: &mut Scanner) -> Result<Self, Error> {
        let format = block.format().ok_or_else(|| format_err!("invalid format"))?;
        let link = block.extent_index();
        Ok(match format {
            PropertyFormat::String => {
                let length = Some(block.total_length());
                ScannedPayload::String { length, link }
            }
            PropertyFormat::Bytes => {
                let length = block.total_length();
                ScannedPayload::Bytes { length, link }
            }
            PropertyFormat::StringReference => ScannedPayload::String { link, length: None },
        })
    }
}

impl BuildScannedPayload<Array<Unknown>> for ScannedPayload {
    fn build(
        block: ScannedBlock<'_, Array<Unknown>>,
        _scanner: &mut Scanner,
    ) -> Result<Self, Error> {
        let entry_type =
            block.entry_type().ok_or_else(|| format_err!("unknown array entry type"))?;
        let array_format = block.format().ok_or_else(|| format_err!("unknown array format"))?;
        let slots = block.slots();
        match entry_type {
            BlockType::IntValue => {
                let block = block.cast_array::<Int>().unwrap();
                let numbers: Result<Vec<i64>, _> = (0..slots)
                    .map(|i| block.get(i).ok_or_else(|| format_err!("no entry at index: {i}")))
                    .collect();
                Ok(ScannedPayload::IntArray(numbers?, array_format))
            }
            BlockType::UintValue => {
                let block = block.cast_array::<Uint>().unwrap();
                let numbers: Result<Vec<u64>, _> = (0..slots)
                    .map(|i| block.get(i).ok_or_else(|| format_err!("no entry at index: {i}")))
                    .collect();
                Ok(ScannedPayload::UintArray(numbers?, array_format))
            }
            BlockType::DoubleValue => {
                let block = block.cast_array::<Double>().unwrap();
                let numbers: Result<Vec<f64>, _> = (0..slots)
                    .map(|i| block.get(i).ok_or_else(|| format_err!("no entry at index: {i}")))
                    .collect();
                Ok(ScannedPayload::DoubleArray(numbers?, array_format))
            }
            BlockType::StringReference => {
                let block = block.cast_array::<StringRef>().unwrap();
                let indexes: Result<Vec<BlockIndex>, _> = (0..slots)
                    .map(|i| {
                        block
                            .get_string_index_at(i)
                            .ok_or_else(|| format_err!("no entry at index: {i}"))
                    })
                    .collect();
                Ok(ScannedPayload::StringArray(indexes?))
            }
            illegal_type => {
                Err(format_err!("No way I should see {:?} for ArrayEntryType", illegal_type))
            }
        }
    }
}

impl BuildScannedPayload<Link> for ScannedPayload {
    fn build(block: ScannedBlock<'_, Link>, scanner: &mut Scanner) -> Result<Self, Error> {
        let child_name = scanner
            .lookup_name_or_string_reference(block.content_index())
            .context(format_err!("Child name not found for LinkValue block {}.", block.index()))?;
        let child_trees = scanner
            .child_trees
            .as_mut()
            .ok_or_else(|| format_err!("LinkValue encountered without child tree."))?;
        let child_tree = child_trees.remove(&child_name).ok_or_else(|| {
            format_err!(
                "Lazy node not found for LinkValue block {} with name {}.",
                block.index(),
                child_name
            )
        })?;
        Ok(ScannedPayload::Link {
            disposition: block
                .link_node_disposition()
                .ok_or_else(|| format_err!("invalid disposition"))?,
            scanned_tree: Box::new(Scanner::try_from(child_tree)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use fidl_diagnostics_validate::*;
    use fuchsia_inspect::reader::snapshot::BackingBuffer;
    use inspect_format::{
        Block, BlockAccessorExt, BlockAccessorMutExt, HeaderFields, PayloadFields, ReadBytes,
        constants,
    };
    use num_traits::ToPrimitive;

    // TODO(https://fxbug.dev/42115894): Depending on the resolution of https://fxbug.dev/42115938, move this const out of mod test.
    const MAX_BLOCK_BITS: usize = constants::MAX_ORDER_SIZE * BITS_PER_BYTE;

    fn copy_into(source: &[u8], dest: &mut [u8], offset: usize) {
        dest[offset..offset + source.len()].copy_from_slice(source);
    }

    // Run "fx test inspect-validator-test -- --nocapture" to see all the output
    // and verify you're getting appropriate error messages for each tweaked byte.
    // (The alternative is hard-coding expected error strings, which is possible but ugh.)
    fn try_byte(
        buffer: &mut [u8],
        (index, offset): (usize, usize),
        value: u8,
        predicted: Option<&str>,
    ) {
        let location = index * 16 + offset;
        let previous = buffer[location];
        buffer[location] = value;
        let actual = data::Scanner::try_from(buffer as &[u8]).map(|d| d.data().to_string());

        match (&predicted, &actual) {
            (None, Err(actual)) => {
                println!("With ({index},{offset}) -> {value}, got expected error {actual:?}");
            }
            (None, Ok(actual)) => {
                println!(
                    "BAD: With ({},{}) -> {}, expected error but got string {:?}",
                    index, offset, value, actual
                );
            }
            (Some(_), Err(actual)) => {
                println!(
                    "BAD: With ({index},{offset}) -> {value}, got unexpected error {actual:?}"
                );
            }
            (Some(predicted), Ok(actual)) => {
                if predicted == actual {
                    println!(
                        "With ({},{}) -> {}, got expected string {:?}",
                        index, offset, value, predicted
                    );
                } else {
                    println!(
                        "BAD: With ({},{}) -> {}, expected string {:?} but got {:?}",
                        index, offset, value, predicted, actual
                    );
                    println!("Raw data: {:?}", data::Scanner::try_from(buffer as &[u8]))
                }
            }
        }
        assert_eq!(predicted, actual.as_ref().ok().map(|s| &s[..]));
        buffer[location] = previous;
    }

    fn put_header<T: ReadBytes>(header: &Block<&mut T, Unknown>, buffer: &mut [u8], index: usize) {
        copy_into(&HeaderFields::value(header).to_le_bytes(), buffer, index * 16);
    }

    fn put_payload<T: ReadBytes>(
        payload: &Block<&mut T, Unknown>,
        buffer: &mut [u8],
        index: usize,
    ) {
        copy_into(&PayloadFields::value(payload).to_le_bytes(), buffer, index * 16 + 8);
    }

    #[fuchsia::test]
    fn test_scanning_string_reference() {
        let mut buffer = [0u8; 4096];
        const NODE: BlockIndex = BlockIndex::new(3);
        const NUMBER_NAME: BlockIndex = BlockIndex::new(4);
        const NUMBER_EXTENT: BlockIndex = BlockIndex::new(5);

        // VMO Header block (index 0)
        let mut container = [0u8; 16];
        let mut header = container.block_at_mut(BlockIndex::EMPTY);
        HeaderFields::set_order(&mut header, 0);
        HeaderFields::set_block_type(&mut header, BlockType::Header.to_u8().unwrap());
        HeaderFields::set_header_magic(&mut header, constants::HEADER_MAGIC_NUMBER);
        HeaderFields::set_header_version(&mut header, constants::HEADER_VERSION_NUMBER);
        put_header(&header, &mut buffer, (*BlockIndex::HEADER).try_into().unwrap());

        // create a Node named number
        HeaderFields::set_order(&mut header, 0);
        HeaderFields::set_block_type(&mut header, BlockType::NodeValue.to_u8().unwrap());
        HeaderFields::set_value_name_index(&mut header, *NUMBER_NAME);
        HeaderFields::set_value_parent_index(&mut header, *BlockIndex::HEADER);
        put_header(&header, &mut buffer, (*NODE).try_into().unwrap());

        // create a STRING_REFERENCE with value "number" that is the above Node's name.
        HeaderFields::set_order(&mut header, 0);
        HeaderFields::set_block_type(&mut header, BlockType::StringReference.to_u8().unwrap());
        HeaderFields::set_extent_next_index(&mut header, *NUMBER_EXTENT);
        put_header(&header, &mut buffer, (*NUMBER_NAME).try_into().unwrap());
        copy_into(&[6, 0, 0, 0], &mut buffer, (*NUMBER_NAME * 16 + 8).try_into().unwrap());
        copy_into(b"numb", &mut buffer, (*NUMBER_NAME * 16 + 12).try_into().unwrap());
        let mut container = [0u8; 16];
        let mut number_extent = container.block_at_mut(BlockIndex::EMPTY);
        HeaderFields::set_order(&mut number_extent, 0);
        HeaderFields::set_block_type(&mut number_extent, BlockType::Extent.to_u8().unwrap());
        HeaderFields::set_extent_next_index(&mut number_extent, 0);
        put_header(&number_extent, &mut buffer, (*NUMBER_EXTENT).try_into().unwrap());
        copy_into(b"er", &mut buffer, (*NUMBER_EXTENT * 16 + 8).try_into().unwrap());

        try_byte(&mut buffer, (16, 0), 0, Some("root ->\n> number ->"));
    }

    #[fuchsia::test]
    fn test_scanning_logic() {
        let mut buffer = [0u8; 4096];
        // VMO Header block (index 0)
        const HEADER: usize = 0;
        {
            let mut container = [0u8; 16];
            let mut header = container.block_at_mut(BlockIndex::EMPTY);
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::Header.to_u8().unwrap());
            HeaderFields::set_header_magic(&mut header, constants::HEADER_MAGIC_NUMBER);
            HeaderFields::set_header_version(&mut header, constants::HEADER_VERSION_NUMBER);
            put_header(&header, &mut buffer, HEADER);
        }
        const ROOT: usize = 1;
        {
            let mut container = [0u8; 16];
            let mut header = container.block_at_mut(BlockIndex::EMPTY);
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::NodeValue.to_u8().unwrap());
            HeaderFields::set_value_name_index(&mut header, 2);
            HeaderFields::set_value_parent_index(&mut header, 0);
            put_header(&header, &mut buffer, ROOT);
        }

        // Root's Name block
        const ROOT_NAME: usize = 2;
        {
            let mut container = [0u8; 16];
            let mut header = container.block_at_mut(BlockIndex::EMPTY);
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::Name.to_u8().unwrap());
            HeaderFields::set_name_length(&mut header, 4);
            put_header(&header, &mut buffer, ROOT_NAME);
        }
        copy_into(b"node", &mut buffer, ROOT_NAME * 16 + 8);
        try_byte(&mut buffer, (16, 0), 0, Some("root ->\n> node ->"));
        // Mess up HEADER_MAGIC_NUMBER - it should fail to load.
        try_byte(&mut buffer, (HEADER, 7), 0, None);
        // Mess up node's parent; should disappear.
        try_byte(&mut buffer, (ROOT, 1), 1, Some("root ->"));
        // Mess up root's name; should fail.
        try_byte(&mut buffer, (ROOT, 5), 1, None);
        // Mess up generation count; should fail (and not hang).
        try_byte(&mut buffer, (HEADER, 8), 1, None);
        // But an even generation count should work.
        try_byte(&mut buffer, (HEADER, 8), 2, Some("root ->\n> node ->"));

        // Let's give it a property.
        const NUMBER: usize = 4;
        let mut container = [0u8; 16];
        let mut number_header = container.block_at_mut(BlockIndex::EMPTY);
        HeaderFields::set_order(&mut number_header, 0);
        HeaderFields::set_block_type(&mut number_header, BlockType::IntValue.to_u8().unwrap());
        HeaderFields::set_value_name_index(&mut number_header, 3);
        HeaderFields::set_value_parent_index(&mut number_header, 1);
        put_header(&number_header, &mut buffer, NUMBER);
        const NUMBER_NAME: usize = 3;
        {
            let mut container = [0u8; 16];
            let mut header = container.block_at_mut(BlockIndex::EMPTY);
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::Name.to_u8().unwrap());
            HeaderFields::set_name_length(&mut header, 6);
            put_header(&header, &mut buffer, NUMBER_NAME);
            copy_into(b"number", &mut buffer, NUMBER_NAME * 16 + 8);
        }

        try_byte(&mut buffer, (HEADER, 8), 2, Some("root ->\n> node ->\n> > number: Int(0)"));
        try_byte(&mut buffer, (NUMBER, 1), 5, Some("root ->\n> node ->\n> > number: Uint(0)"));
        try_byte(&mut buffer, (NUMBER, 1), 6, Some("root ->\n> node ->\n> > number: Double(0.0)"));
        try_byte(&mut buffer, (NUMBER, 1), 7, Some("root ->\n> node ->\n> > number: String(\"\")"));
        // Array block will have illegal Array Entry Type of 0.
        try_byte(&mut buffer, (NUMBER, 1), 0xb0, None);
        // 15 is an illegal block type.
        try_byte(&mut buffer, (NUMBER, 1), 0xf, None);
        HeaderFields::set_order(&mut number_header, 2);
        HeaderFields::set_block_type(&mut number_header, BlockType::ArrayValue.to_u8().unwrap());
        put_header(&number_header, &mut buffer, NUMBER);
        // Array block again has illegal Array Entry Type of 0.
        try_byte(&mut buffer, (128, 0), 0, None);
        // 4, 5, and 6 are legal array types.
        try_byte(
            &mut buffer,
            (NUMBER, 8),
            0x04,
            Some("root ->\n> node ->\n> > number: IntArray([], Default)"),
        );
        try_byte(
            &mut buffer,
            (NUMBER, 8),
            0x05,
            Some("root ->\n> node ->\n> > number: UintArray([], Default)"),
        );
        try_byte(
            &mut buffer,
            (NUMBER, 8),
            0x06,
            Some("root ->\n> node ->\n> > number: DoubleArray([], Default)"),
        );
        // 0, 1, and 2 are legal formats.
        try_byte(
            &mut buffer,
            (NUMBER, 8),
            0x14,
            Some("root ->\n> node ->\n> > number: IntArray([], LinearHistogram)"),
        );
        try_byte(
            &mut buffer,
            (NUMBER, 8),
            0x24,
            Some("root ->\n> node ->\n> > number: IntArray([], ExponentialHistogram)"),
        );
        try_byte(&mut buffer, (NUMBER, 8), 0x34, None);
        // Let's make sure other Value block-type numbers are rejected.
        try_byte(&mut buffer, (NUMBER, 8), BlockType::ArrayValue.to_u8().unwrap(), None);
        buffer[NUMBER * 16 + 8] = 4; // Int, Default
        buffer[NUMBER * 16 + 9] = 2; // 2 entries
        try_byte(
            &mut buffer,
            (NUMBER, 16),
            42,
            Some("root ->\n> node ->\n> > number: IntArray([42, 0], Default)"),
        );
        try_byte(
            &mut buffer,
            (NUMBER, 24),
            42,
            Some("root ->\n> node ->\n> > number: IntArray([0, 42], Default)"),
        );
    }

    #[fuchsia::test]
    async fn test_to_string_order() -> Result<(), Error> {
        // Make sure property payloads are distinguished by name, value, and type
        // but ignore id and parent, and that prefix is used.
        let int0 = Property {
            name: "int0".into(),
            id: 2.into(),
            parent: 1.into(),
            payload: Payload::Int(0),
        }
        .to_string("");
        let int1_struct = Property {
            name: "int1".into(),
            id: 2.into(),
            parent: 1.into(),
            payload: Payload::Int(1),
        };
        let int1 = int1_struct.to_string("");
        assert_ne!(int0, int1);
        let uint0 = Property {
            name: "uint0".into(),
            id: 2.into(),
            parent: 1.into(),
            payload: Payload::Uint(0),
        }
        .to_string("");
        assert_ne!(int0, uint0);
        let int0_different_name = Property {
            name: "int0_different_name".into(),
            id: 2.into(),
            parent: 1.into(),
            payload: Payload::Int(0),
        }
        .to_string("");
        assert_ne!(int0, int0_different_name);
        let uint0_different_ids = Property {
            name: "uint0".into(),
            id: 3.into(),
            parent: 4.into(),
            payload: Payload::Uint(0),
        }
        .to_string("");
        assert_eq!(uint0, uint0_different_ids);
        let int1_different_prefix = int1_struct.to_string("foo");
        assert_ne!(int1, int1_different_prefix);
        // Test that order doesn't matter. Use a real VMO rather than Data's
        // HashMaps which may not reflect order of addition.
        let mut puppet1 = puppet::tests::local_incomplete_puppet().await?;
        let mut child1_action = create_node!(parent:0, id:1, name:"child1");
        let mut child2_action = create_node!(parent:0, id:2, name:"child2");
        let mut property1_action =
            create_numeric_property!(parent:0, id:1, name:"prop1", value: Value::IntT(1));
        let mut property2_action =
            create_numeric_property!(parent:0, id:2, name:"prop2", value: Value::IntT(2));
        puppet1.apply(&mut child1_action).await?;
        puppet1.apply(&mut child2_action).await?;
        let mut puppet2 = puppet::tests::local_incomplete_puppet().await?;
        puppet2.apply(&mut child2_action).await?;
        puppet2.apply(&mut child1_action).await?;
        assert_eq!(puppet1.read_data().await?.to_string(), puppet2.read_data().await?.to_string());
        puppet1.apply(&mut property1_action).await?;
        puppet1.apply(&mut property2_action).await?;
        puppet2.apply(&mut property2_action).await?;
        puppet2.apply(&mut property1_action).await?;
        assert_eq!(puppet1.read_data().await?.to_string(), puppet2.read_data().await?.to_string());
        // Make sure the tree distinguishes based on node position
        puppet1 = puppet::tests::local_incomplete_puppet().await?;
        puppet2 = puppet::tests::local_incomplete_puppet().await?;
        let mut subchild2_action = create_node!(parent:1, id:2, name:"child2");
        puppet1.apply(&mut child1_action).await?;
        puppet2.apply(&mut child1_action).await?;
        puppet1.apply(&mut child2_action).await?;
        puppet2.apply(&mut subchild2_action).await?;
        assert_ne!(puppet1.read_data().await?.to_string(), puppet2.read_data().await?.to_string());
        // ... and property position
        let mut subproperty2_action =
            create_numeric_property!(parent:1, id:2, name:"prop2", value: Value::IntT(1));
        puppet1.apply(&mut child1_action).await?;
        puppet2.apply(&mut child1_action).await?;
        puppet1.apply(&mut property2_action).await?;
        puppet2.apply(&mut subproperty2_action).await?;
        Ok(())
    }

    #[fuchsia::test]
    fn test_bit_ops() -> Result<(), Error> {
        assert_eq!(low_bits(0xff, 3), 7);
        assert_eq!(low_bits(0x04, 3), 4);
        assert_eq!(low_bits(0xf8, 3), 0);
        assert_eq!(low_bits(0xab, 99), 0xab);
        assert_eq!(low_bits(0xff, 0), 0);
        assert_eq!(high_bits(0xff, 3), 0xe0);
        assert_eq!(high_bits(0x20, 3), 0x20);
        assert_eq!(high_bits(0x1f, 3), 0);
        assert_eq!(high_bits(0xab, 99), 0xab);
        assert_eq!(high_bits(0xff, 0), 0);
        Ok(())
    }

    #[fuchsia::test]
    fn test_zero_bits() -> Result<(), Error> {
        let mut buffer = [0u8; 48];
        for byte in buffer.iter_mut().take(16) {
            *byte = 0xff;
        }
        for byte in buffer.iter_mut().skip(32) {
            *byte = 0xff;
        }
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 1, 0).is_err());
            assert!(check_zero_bits(&buffer, &block, 0, 0).is_ok());
            assert!(check_zero_bits(&buffer, &block, 0, MAX_BLOCK_BITS).is_ok());
        }
        // Don't mess with buffer[0]; that defines block size and type.
        // The block I'm testing (index 1) is in between two all-ones blocks.
        // Its bytes are thus 16..23 in the buffer.
        buffer[1 + 16] = 1;
        // Now bit 8 of the block is 1. Checking any range that includes bit 8 should give an
        // error (even single-bit 8...8). Other ranges should succeed.
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 8, 8).is_err());
            assert!(check_zero_bits(&buffer, &block, 8, MAX_BLOCK_BITS).is_err());
            assert!(check_zero_bits(&buffer, &block, 9, MAX_BLOCK_BITS).is_ok());
        }
        buffer[2 + 16] = 0x80;
        // Now bits 8 and 23 are 1. The range 9...MAX_BLOCK_BITS that succeeded before should fail.
        // 9...22 and 24...MAX_BLOCK_BITS should succeed. So should 24...63.
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 9, MAX_BLOCK_BITS).is_err());
            assert!(check_zero_bits(&buffer, &block, 9, 22).is_ok());
            assert!(check_zero_bits(&buffer, &block, 24, MAX_BLOCK_BITS).is_ok());
            assert!(check_zero_bits(&buffer, &block, 24, 63).is_ok());
        }
        buffer[2 + 16] = 0x20;
        // Now bits 8 and 21 are 1. This tests bit-checks in the middle of the byte.
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 16, 20).is_ok());
            assert!(check_zero_bits(&buffer, &block, 21, 21).is_err());
            assert!(check_zero_bits(&buffer, &block, 22, 63).is_ok());
        }
        buffer[7 + 16] = 0x80;
        // Now bits 8, 21, and 63 are 1. Checking 22...63 should fail; 22...62 should succeed.
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 22, 63).is_err());
            assert!(check_zero_bits(&buffer, &block, 22, 62).is_ok());
        }
        buffer[3 + 16] = 0x10;
        // Here I'm testing whether 1 bits in the bytes between the ends of the range are also
        // detected (cause the check to fail) (to make sure my loop doesn't have an off by 1 error).
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 22, 62).is_err());
        }
        buffer[3 + 16] = 0;
        buffer[4 + 16] = 0x10;
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 22, 62).is_err());
        }
        buffer[4 + 16] = 0;
        buffer[5 + 16] = 0x10;
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 22, 62).is_err());
        }
        buffer[5 + 16] = 0;
        buffer[6 + 16] = 0x10;
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 22, 62).is_err());
        }
        buffer[1 + 16] = 0x81;
        // Testing whether I can correctly ignore 1 bits within a single byte that are outside
        // the specified range, and detect 1 bits that are inside the range.
        {
            let backing_buffer = BackingBuffer::from(buffer.to_vec());
            let block = backing_buffer.block_at(1.into());
            assert!(check_zero_bits(&buffer, &block, 9, 14).is_ok());
            assert!(check_zero_bits(&buffer, &block, 8, 14).is_err());
            assert!(check_zero_bits(&buffer, &block, 9, 15).is_err());
        }
        Ok(())
    }

    #[fuchsia::test]
    fn test_reserved_fields() {
        let mut buffer = [0u8; 4096];
        // VMO Header block (index 0)
        const HEADER: usize = 0;
        {
            let mut container = [0u8; 16];
            let mut header = container.block_at_mut(BlockIndex::EMPTY);
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::Header.to_u8().unwrap());
            HeaderFields::set_header_magic(&mut header, constants::HEADER_MAGIC_NUMBER);
            HeaderFields::set_header_version(&mut header, constants::HEADER_VERSION_NUMBER);
            put_header(&header, &mut buffer, HEADER);
        }
        const VALUE: usize = 1;
        let mut container = [0u8; 16];
        let mut value_header = container.block_at_mut(BlockIndex::EMPTY);
        HeaderFields::set_order(&mut value_header, 0);
        HeaderFields::set_block_type(&mut value_header, BlockType::NodeValue.to_u8().unwrap());
        HeaderFields::set_value_name_index(&mut value_header, 2);
        HeaderFields::set_value_parent_index(&mut value_header, 0);
        put_header(&value_header, &mut buffer, VALUE);
        // Root's Name block
        const VALUE_NAME: usize = 2;
        {
            let mut buf = [0; 16];
            let mut header = buf.block_at_mut(0.into());
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::Name.to_u8().unwrap());
            HeaderFields::set_name_length(&mut header, 5);
            put_header(&header, &mut buffer, VALUE_NAME);
        }
        copy_into(b"value", &mut buffer, VALUE_NAME * 16 + 8);
        // Extent block (not linked into tree)
        const EXTENT: usize = 3;
        {
            let mut container = [0u8; 16];
            let mut header = container.block_at_mut(BlockIndex::EMPTY);
            HeaderFields::set_order(&mut header, 0);
            HeaderFields::set_block_type(&mut header, BlockType::Extent.to_u8().unwrap());
            HeaderFields::set_extent_next_index(&mut header, 0);
            put_header(&header, &mut buffer, EXTENT);
        }
        // Let's make sure it scans.
        try_byte(&mut buffer, (16, 0), 0, Some("root ->\n> value ->"));
        // Put garbage in a random FREE block body - should fail.
        // TODO(https://fxbug.dev/42115894): Depending on the resolution of https://fxbug.dev/42115938, uncomment or delete this test.
        //try_byte(&mut buffer, (6, 9), 42, None);
        // Put garbage in a random FREE block header - should be fine.
        try_byte(&mut buffer, (6, 7), 42, Some("root ->\n> value ->"));
        // Put garbage in NAME header - should fail.
        try_byte(&mut buffer, (VALUE_NAME, 7), 42, None);
        // Put garbage in EXTENT header - should fail.
        try_byte(&mut buffer, (EXTENT, 6), 42, None);
        HeaderFields::set_block_type(&mut value_header, BlockType::ArrayValue.to_u8().unwrap());
        put_header(&value_header, &mut buffer, VALUE);
        {
            let mut container = [0u8; 16];
            let mut array_subheader = container.block_at_mut(BlockIndex::EMPTY);
            PayloadFields::set_array_entry_type(
                &mut array_subheader,
                BlockType::IntValue.to_u8().unwrap(),
            );
            PayloadFields::set_array_flags(
                &mut array_subheader,
                ArrayFormat::Default.to_u8().unwrap(),
            );
            put_payload(&array_subheader, &mut buffer, VALUE);
        }
        try_byte(&mut buffer, (16, 0), 0, Some("root ->\n> value: IntArray([], Default)"));
        // Put garbage in reserved part of Array spec, should fail.
        try_byte(&mut buffer, (VALUE, 12), 42, None);
        HeaderFields::set_block_type(&mut value_header, BlockType::IntValue.to_u8().unwrap());
        put_header(&value_header, &mut buffer, VALUE);
        // Now the array spec is just a (large) value; it should succeed.
        try_byte(&mut buffer, (VALUE, 12), 42, Some("root ->\n> value: Int(180388626436)"));
    }
}
