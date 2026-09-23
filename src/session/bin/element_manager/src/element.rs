// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::{DiscoverableProtocolMarker, Proxy};
use fuchsia_async as fasync;

/// Represents a component launched by an Element Manager.
///
/// The Element can be used to connect to services exposed by the underlying component.
#[derive(Debug)]
pub struct Element {
    /// A `Directory` request channel for requesting services exposed by the component.
    exposed_capabilities: zx::Channel,

    /// The component URL used to launch the component.
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    url: String,

    /// Component child name, or empty string if not a child of the realm.
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    name: String,

    /// Component child collection name or empty string if not a child of the realm.
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    collection: String,

    // Whether or not the child exposes fuchsia.ui.app.ViewProvider.
    #[allow(unused)]
    use_view_provider: bool,
}

/// A component launched in response to `ElementManager::ProposeElement()`.
///
/// A session uses `ElementManager` to launch and return the Element, and can then use the Element
/// to connect to exposed capabilities.
impl Element {
    /// Creates an Element from a component's exposed capabilities directory.
    ///
    /// # Parameters
    /// - `directory_channel`: A channel to the component's `Directory` of exposed capabilities.
    /// - `name`: The launched component's name.
    /// - `url`: The launched component URL.
    /// - `collection`: The launched component's collection name.
    pub fn from_directory_channel(
        exposed_capabilities: zx::Channel,
        name: &str,
        url: &str,
        collection: &str,
        use_view_provider: bool,
    ) -> Element {
        Element {
            exposed_capabilities,
            url: url.to_string(),
            name: name.to_string(),
            collection: collection.to_string(),
            use_view_provider,
        }
    }

    #[allow(unused)]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[allow(unused)]
    pub fn collection(&self) -> &str {
        &self.collection
    }

    #[allow(unused)]
    pub fn use_view_provider(&self) -> bool {
        self.use_view_provider
    }

    // # Note
    //
    // The methods below are copied from fuchsia_component::client::App in order to offer
    // services in the same way, but from any `Element`.

    /// Returns a reference to the component's `Directory` of exposed capabilities. A session can
    /// request services, and/or other capabilities, from the Element, using this channel.
    ///
    /// # Returns
    /// A `channel` to the component's `Directory` of exposed capabilities.
    #[inline]
    pub fn directory_channel(&self) -> &zx::Channel {
        &self.exposed_capabilities
    }

    /// Connect to a protocol provided by the `Element`.
    ///
    /// # Type Parameters
    /// - P: A FIDL service `Marker` type.
    ///
    /// # Returns
    /// - A service `Proxy` matching the `Marker`, or an error if the service is not available from
    /// the `Element`.
    #[inline]
    pub fn connect_to_protocol<P: DiscoverableProtocolMarker>(&self) -> Result<P::Proxy, Error> {
        let (client_channel, server_channel) = zx::Channel::create();
        self.connect_to_protocol_with_channel::<P>(server_channel)?;
        Ok(P::Proxy::from_channel(fasync::Channel::from_channel(client_channel)))
    }

    /// Connect to a protocol by passing a channel for the server.
    ///
    /// # Type Parameters
    /// - P: A FIDL service `Marker` type.
    ///
    /// # Parameters
    /// - server_channel: The server-side endpoint of a channel pair, to bind to the requested
    /// service. The caller will interact with the service via the client-side endpoint.
    ///
    /// # Returns
    /// - Result::Ok or an error if the service is not available from the `Element`.
    #[inline]
    pub fn connect_to_protocol_with_channel<P: DiscoverableProtocolMarker>(
        &self,
        server_channel: zx::Channel,
    ) -> Result<(), Error> {
        self.connect_to_named_protocol_with_channel(P::PROTOCOL_NAME, server_channel)
    }

    /// Connect to a protocol by name.
    ///
    /// # Parameters
    /// - service_name: A FIDL service by name.
    /// - server_channel: The server-side endpoint of a channel pair, to bind to the requested
    /// service. The caller will interact with the service via the client-side endpoint.
    ///
    /// # Returns
    /// - Result::Ok or an error if the service is not available from the `Element`.
    #[inline]
    pub fn connect_to_named_protocol_with_channel(
        &self,
        protocol_name: &str,
        server_channel: zx::Channel,
    ) -> Result<(), Error> {
        fdio::service_connect_at(self.directory_channel(), protocol_name, server_channel)?;
        Ok(())
    }
}
