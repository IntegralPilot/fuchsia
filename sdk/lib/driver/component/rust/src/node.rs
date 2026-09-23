// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ClientEnd;
use fidl_fuchsia_driver_framework::{
    NodeAddArgs, NodeControllerMarker, NodeMarker, NodeProperty2, NodeProxy, Offer,
};
use log::error;
use zx::Status;

mod offers;
mod properties;

pub use offers::*;
pub use properties::*;

/// Holds on to a [`NodeProxy`] and provides simplified methods for adding child nodes.
pub struct Node(NodeProxy);

impl Node {
    /// Adds an owned child node to this node and returns the [`ClientEnd`]s for its
    /// `NodeController` and `Node`. Use a [`NodeBuilder`] to create the `args` argument.
    ///
    /// If you don't need the `NodeController`, it is safe to drop it, but the node will be removed
    /// if the client end for the `Node` is dropped.
    ///
    /// Logs an error message and returns [`Status::INTERNAL`] if there's an error adding the
    /// child node.
    pub async fn add_owned_child(
        &self,
        args: NodeAddArgs,
    ) -> Result<(ClientEnd<NodeMarker>, ClientEnd<NodeControllerMarker>), Status> {
        let (child_controller, child_controller_server) = fidl::endpoints::create_endpoints();
        let (child_node, child_node_server) = fidl::endpoints::create_endpoints();
        self.proxy()
            .add_child(args, child_controller_server, Some(child_node_server))
            .await
            .map_err(|err| {
                error!("transport error trying to create child node: {err}");
                Status::INTERNAL
            })?
            .map_err(|err| {
                error!("failed to create child node: {err:?}");
                Status::INTERNAL
            })?;
        Ok((child_node, child_controller))
    }

    /// Adds an owned child node to this node and returns the [`ClientEnd`]s for its
    /// `NodeController`. Use a [`NodeBuilder`] to create the `args` argument.
    ///
    /// If you don't need the `NodeController`, it is safe to drop it. The driver runtime will
    /// attempt to find a driver to bind the node to.
    ///
    /// Logs an error message and returns [`Status::INTERNAL`] if there's an error adding the
    /// child node.
    pub async fn add_child(
        &self,
        args: NodeAddArgs,
    ) -> Result<ClientEnd<NodeControllerMarker>, Status> {
        let (child_controller, child_controller_server) = fidl::endpoints::create_endpoints();
        self.proxy()
            .add_child(args, child_controller_server, None)
            .await
            .map_err(|err| {
                error!("transport error trying to create child node: {err}");
                Status::INTERNAL
            })?
            .map_err(|err| {
                error!("failed to create child node: {err:?}");
                Status::INTERNAL
            })?;
        Ok(child_controller)
    }

    /// Accesses the underlying [`NodeProxy`].
    pub fn proxy(&self) -> &NodeProxy {
        &self.0
    }
}

impl From<NodeProxy> for Node {
    fn from(value: NodeProxy) -> Self {
        Node(value)
    }
}

/// A builder for adding a child node to an existing [`Node`].
pub struct NodeBuilder(NodeAddArgs);

impl NodeBuilder {
    /// Creates a new [`NodeAddBuilder`] with the given `node_name` already set.
    pub fn new(node_name: impl Into<String>) -> Self {
        Self(NodeAddArgs { name: Some(node_name.into()), ..Default::default() })
    }

    /// Adds a property to the node. The `key` argument is something that can convert to a
    /// [`PropertyKey`], which includes strings and [`u32`] integers. The `value` argument is
    /// something that can convert into a [`PropertyValue`], which includes strings, [`u32`]
    /// integers, and [`bool`] values.
    pub fn add_property(mut self, key: impl Into<String>, value: impl Into<PropertyValue>) -> Self {
        let key = key.into();
        let value = value.into().0;
        self.0.properties2.get_or_insert_with(Vec::new).push(NodeProperty2 { key, value });
        self
    }

    /// Adds a service offer to the node. The `offer` can be built with the
    /// [`offers::ZirconServiceOffer`] builder.
    pub fn add_offer(mut self, offer: Offer) -> Self {
        self.0.offers2.get_or_insert_with(Vec::new).push(offer);
        self
    }

    /// Triggers the callback if |condition| is true.
    pub fn pipe_if<F>(self, condition: bool, callback: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        if condition { callback(self) } else { self }
    }

    /// Triggers the callback if |value| has a some value in it.
    pub fn pipe_opt<T, F>(self, value: Option<T>, callback: F) -> Self
    where
        F: FnOnce(Self, T) -> Self,
    {
        if let Some(value) = value { callback(self, value) } else { self }
    }

    /// Finalize the construction of the node for use with [`Node::add_child`] or
    /// [`Node::add_owned_child`].
    pub fn build(self) -> NodeAddArgs {
        self.0
    }
}
