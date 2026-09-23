// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::directory::FatDirectory;
use crate::file::FatFile;
use std::ops::Deref;
use std::sync::{Arc, Weak};
use zx::Status;

pub trait Node {
    /// Attach this FatNode to the given FatDirectory, with the given name.
    fn attach(&self, parent: Arc<FatDirectory>, name: &str) -> Result<(), Status>;

    /// Detach this FatNode from its parent.
    fn detach(&self);

    /// Takes an open count and opens the underlying node if not already open.
    fn open_ref(&self) -> Result<(), Status>;

    /// Releases an open count.
    fn close_ref(&self);

    /// Close the underlying node and all of its children, regardless of the number of open
    /// connections.
    fn shut_down(&self) -> Result<(), Status>;

    /// Flushes the directory entry for this node.
    fn flush_dir_entry(&self) -> Result<(), Status>;

    /// Called when the node has been successfully deleted.
    fn did_delete(&self);
}

#[derive(Clone, Debug)]
/// This enum is used to represent values which could be either a FatDirectory
/// or a FatFile. This holds a strong reference to the contained file/directory.
pub enum FatNode {
    Dir(Arc<FatDirectory>),
    File(Arc<FatFile>),
}

impl FatNode {
    /// Downgrade this FatNode into a WeakFatNode.
    pub fn downgrade(&self) -> WeakFatNode {
        match self {
            FatNode::Dir(a) => WeakFatNode::Dir(Arc::downgrade(a)),
            FatNode::File(b) => WeakFatNode::File(Arc::downgrade(b)),
        }
    }

    pub fn as_node(&self) -> &(dyn Node + 'static) {
        match self {
            FatNode::Dir(a) => a.as_ref() as &dyn Node,
            FatNode::File(b) => b.as_ref() as &dyn Node,
        }
    }
}

impl<'a> Deref for FatNode {
    type Target = dyn Node;

    fn deref(&self) -> &Self::Target {
        self.as_node()
    }
}

/// The same as FatNode, but using a weak reference.
#[derive(Debug)]
pub enum WeakFatNode {
    Dir(Weak<FatDirectory>),
    File(Weak<FatFile>),
}

impl WeakFatNode {
    /// Try and upgrade this WeakFatNode to a FatNode. Returns None
    /// if the referenced object has been destroyed.
    pub fn upgrade(&self) -> Option<FatNode> {
        match self {
            WeakFatNode::Dir(a) => a.upgrade().map(|val| FatNode::Dir(val)),
            WeakFatNode::File(b) => b.upgrade().map(|val| FatNode::File(val)),
        }
    }
}

/// RAII class that will close nodes when dropped.  This class is useful
/// for instances where temporary open counts are required.
pub struct Closer {
    nodes: std::vec::Vec<FatNode>,
}

impl Closer {
    pub fn new() -> Self {
        Closer { nodes: Vec::new() }
    }

    pub fn add(&mut self, node: FatNode) -> FatNode {
        self.nodes.push(node.clone());
        node
    }
}

impl Drop for Closer {
    fn drop(&mut self) {
        self.nodes.drain(..).for_each(|n: FatNode| n.close_ref());
    }
}
