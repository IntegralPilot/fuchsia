// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Metadata for [time matrices][`TimeMatrix`].
//!
//! The [`DataSemantic`] of the [`Statistic`] determines which metadata types can be used to
//! annotate a [`TimeMatrix`]. For example, the [`Union`] statistic has a [`Bitset`] semantic that
//! requires [`BitsetIndex`].
//!
//! [`Bitset`]: crate::experimental::series::Bitset
//! [`BitsetIndex`]: crate::experimental::series::metadata::BitsetIndex
//! [`DataSemantic`]: crate::experimental::series::DataSemantic
//! [`Statistic`]: crate::experimental::series::statistic::Statistic
//! [`TimeMatrix`]: crate::experimental::series::TimeMatrix
//! [`Union`]: crate::experimental::series::statistic::Union

use fuchsia_inspect::Node;
use itertools::Itertools;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::marker::PhantomData;

pub trait Metadata {
    fn record(&self, node: &Node);

    fn record_with_parent(&self, node: &Node) {
        node.record_child("metadata", |node| {
            self.record(node);
        });
    }
}

impl Metadata for () {
    fn record(&self, _: &Node) {}
}

/// A textual label for a bit in a [`Bitset`] aggregation.
///
/// [`Bitset`]: crate::experimental::series::Bitset
#[derive(Clone, Debug)]
pub struct BitLabel(Cow<'static, str>);

impl BitLabel {}

impl AsRef<str> for BitLabel {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Display for BitLabel {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<Cow<'static, str>> for BitLabel {
    fn from(label: Cow<'static, str>) -> Self {
        BitLabel(label)
    }
}

impl From<&'static str> for BitLabel {
    fn from(label: &'static str) -> Self {
        BitLabel(label.into())
    }
}

impl From<String> for BitLabel {
    fn from(label: String) -> Self {
        BitLabel(label.into())
    }
}

// TODO(https://fxbug.dev/375475120): It is easiest to construct a `BitsetMap` inline with a
//                                    `Reactor`. This means that the mapping is most likely defined
//                                    far from the data type that represents the bitset. Staleness
//                                    is likely to occur if the mapping or data type change.
//
//                                    Provide an additional mechanism for defining this mapping
//                                    locally to the sampled data type.
/// A map from index to [`BitLabel`]s that indexes a [`Bitset`] aggregation.
///
/// A `BitsetMap` maps labels to particular bits in a bitset. These labels cannot change and are
/// recorded directly in the metadata for a [`TimeMatrix`].
///
/// [`BitLabel`]: crate::experimental::series::metadata::BitLabel
/// [`Bitset`]: crate::experimental::series::Bitset
/// [`TimeMatrix`]: crate::experimental::series::TimeMatrix
#[derive(Clone, Debug)]
pub struct BitsetMap {
    labels: BTreeMap<usize, BitLabel>,
}

impl BitsetMap {
    // TODO(https://fxbug.dev/460232058): Consider merging `BitsetMap` and
    // `DenseBitsetMap`. `DenseBitsetMap` provides behavior similar to
    // `from_ordered` but avoids the hashing and allocations needed to create a
    // BitsetMap.
    pub fn from_ordered<I>(labels: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<BitLabel>,
    {
        BitsetMap { labels: labels.into_iter().map(Into::into).enumerate().collect() }
    }

    pub fn from_indexed<T, I>(labels: I) -> Self
    where
        T: Into<BitLabel>,
        I: IntoIterator<Item = (usize, T)>,
    {
        BitsetMap {
            labels: labels
                .into_iter()
                .unique_by(|(index, _)| *index)
                .map(|(index, label)| (index, label.into()))
                .collect(),
        }
    }

    pub fn record(&self, node: &Node) {
        record_bit_labels_inner(node, self.labels().map(|(index, label)| (*index, label)));
    }

    pub fn labels(&self) -> impl '_ + Iterator<Item = (&usize, &BitLabel)> {
        self.labels.iter()
    }

    pub fn label(&self, index: usize) -> Option<&BitLabel> {
        self.labels.get(&index)
    }
}

/// The actual implementation of recording bit labels.
///
/// This is crate-private so only types in this crate can call it, ensuring
/// uniqueness of each label index.
fn record_bit_labels_inner<I: Iterator<Item = (usize, L)>, L: AsRef<str>>(node: &Node, labels: I) {
    node.record_child("index", |node| {
        for (index, label) in labels
            .filter_map(|(index, label)| u64::try_from(index).ok().map(|index| (index, label)))
        {
            // The index is used as the key, mapping from index to label in the Inspect tree.
            // Inspect sorts keys numerically, so there is no need to format the index.
            node.record_string(index.to_string(), label.as_ref());
        }
    });
}

/// Provides implementation of [`Metadata`]  similar to [`BitsetMap`] that
/// requires a dense (i.e. not sparse) set of bit indices.
pub struct DenseBitsetMap<I, F>(F, PhantomData<I>);

impl<I, F> DenseBitsetMap<I, F> {
    /// Creates a new [`DenseBitsetMap`] from a function that generates labels.
    pub fn new(labels: F) -> Self {
        DenseBitsetMap(labels, PhantomData)
    }
}

impl<I, F> Metadata for DenseBitsetMap<I, F>
where
    I: Iterator,
    I::Item: Into<BitLabel>,
    F: Fn() -> I,
{
    fn record(&self, node: &Node) {
        record_bit_labels_inner(node, self.0().map(Into::into).enumerate());
    }
}

/// A reference to an Inspect node that indexes a [`Bitset`] aggregation.
///
/// A `BitsetNode` provides a path to an Inspect node in which each key represents a bit index and
/// each value represents a label. Unlike [`BitsetMap`], this index is not tightly coupled to a
/// served [`TimeMatrix`] and the index may be dynamic.
///
/// Only the path to the Inspect node is recorded in the metadata for a [`TimeMatrix`].
///
/// [`Bitset`]: crate::experimental::series::Bitset
/// [`BitsetMap`]: crate::experimental::series::metadata::BitsetMap
/// [`TimeMatrix`]: crate::experimental::series::TimeMatrix
#[derive(Clone, Debug)]
pub struct BitsetNode {
    path: Cow<'static, str>,
}

impl BitsetNode {
    pub fn from_path(path: impl Into<Cow<'static, str>>) -> Self {
        BitsetNode { path: path.into() }
    }

    fn record(&self, node: &Node) {
        node.record_string("index_node_path", self.path.as_ref());
    }
}

/// Metadata for a [`Bitset`] aggregation that indexes bits with textual labels.
///
/// [`Bitset`]: crate::experimental::series::Bitset
#[derive(Clone, Debug)]
pub enum BitsetIndex {
    Map(BitsetMap),
    Node(BitsetNode),
}

impl From<BitsetMap> for BitsetIndex {
    fn from(metadata: BitsetMap) -> Self {
        BitsetIndex::Map(metadata)
    }
}

impl From<BitsetNode> for BitsetIndex {
    fn from(metadata: BitsetNode) -> Self {
        BitsetIndex::Node(metadata)
    }
}

impl Metadata for BitsetIndex {
    fn record(&self, node: &Node) {
        match self {
            BitsetIndex::Map(metadata) => metadata.record(node),
            BitsetIndex::Node(metadata) => metadata.record(node),
        }
    }
}
