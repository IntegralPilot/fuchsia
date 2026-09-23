// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations for new Rust bindings.

use anyhow::Context;
use fidl_next::protocol::ServiceConnector as ServiceConnectorTrait;
use fidl_next::{ClientEnd, Discoverable, DiscoverableService, ServerEnd, Service};

use super::{Error, SVC_DIR, connect_channel_to_protocol_at_path};

/// A connector for a FIDL service instance that uses a Zircon channel.
#[repr(transparent)]
pub struct InstanceConnector(zx::Channel);

impl ServiceConnectorTrait<zx::Channel> for InstanceConnector {
    type Error = Error;

    fn connect_to_member(&self, member: &str, server_end: zx::Channel) -> Result<(), Self::Error> {
        fdio::service_connect_at(&self.0, member, server_end).context("failed to connect to member")
    }
}

/// Connect to a FIDL protocol using the provided server end and namespace
/// prefix.
pub fn connect_server_end_to_protocol_at<P: Discoverable>(
    server_end: ServerEnd<P, zx::Channel>,
    service_directory_path: &str,
) -> Result<(), Error> {
    let protocol_path = format!("{}/{}", service_directory_path, P::PROTOCOL_NAME);
    connect_channel_to_protocol_at_path(server_end.into_untyped(), &protocol_path)
}

/// Connect to a FIDL protocol using the provided namespace prefix.
pub fn connect_to_protocol_at<P: Discoverable>(
    service_prefix: &str,
) -> Result<ClientEnd<P, zx::Channel>, Error> {
    let (client_end, server_end) = fidl_next::fuchsia::create_channel();
    let () = connect_server_end_to_protocol_at(server_end, service_prefix)?;
    Ok(client_end)
}

/// Connect to a FIDL protocol in the `/svc` directory of the application's root
/// namespace.
pub fn connect_to_protocol<P: Discoverable>() -> Result<ClientEnd<P, zx::Channel>, Error> {
    connect_to_protocol_at(SVC_DIR)
}

/// Connect to a FIDL service instance in the `/svc` directory of the application's
/// root namespace.
pub fn connect_to_service_instance<S>(instance: &str) -> Result<S::Connector, Error>
where
    S: DiscoverableService + Service<InstanceConnector>,
{
    let service_path = format!("{}/{}/{}", SVC_DIR, S::SERVICE_NAME, instance);
    let (client, server) = zx::Channel::create();
    fuchsia_fs::directory::open_channel_in_namespace(
        &service_path,
        fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY | fidl_fuchsia_io::PERM_READABLE,
        server.into(),
    )?;
    let connector = InstanceConnector(client);
    // SAFETY: The `Service` trait guarantees that `S::Connector` is a
    // `#[repr(transparent)]` wrapper around the generic connector type.
    let connector = std::mem::ManuallyDrop::new(connector);
    Ok(unsafe { std::ptr::read(&*connector as *const InstanceConnector as *const S::Connector) })
}
