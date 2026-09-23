// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fidl::registry::try_from_handle_in_registry;
use crate::{Capability, CapabilityBound, ConversionError, RemoteError, WeakInstanceToken};
use fidl::AsHandleRef;
use fidl_fuchsia_component_sandbox as fsandbox;
use std::sync::Arc;
use vfs::directory::entry::DirectoryEntry;
use vfs::execution_scope::ExecutionScope;

impl crate::fidl::IntoFsandboxCapability for Capability {
    fn into_fsandbox_capability(self, token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        match self {
            Capability::Connector(s) => s.into_fsandbox_capability(token),
            Capability::DirConnector(s) => s.into_fsandbox_capability(token),
            Capability::DictionaryRouter(s) => s.into_fsandbox_capability(token),
            Capability::ConnectorRouter(s) => s.into_fsandbox_capability(token),
            Capability::DirConnectorRouter(s) => s.into_fsandbox_capability(token),
            Capability::DataRouter(s) => s.into_fsandbox_capability(token),
            Capability::Dictionary(s) => s.into_fsandbox_capability(token),
            Capability::Data(s) => s.into_fsandbox_capability(token),
            Capability::Handle(s) => s.into_fsandbox_capability(token),
            Capability::Instance(s) => s.into_fsandbox_capability(token),
        }
    }
}

impl TryFrom<fsandbox::Capability> for Capability {
    type Error = RemoteError;

    /// Converts the FIDL capability back to a Rust Capability.
    ///
    /// In most cases, the Capability was previously inserted into the registry when it
    /// was converted to a FIDL capability. This method takes it out of the registry.
    fn try_from(capability: fsandbox::Capability) -> Result<Self, Self::Error> {
        match capability {
            fsandbox::Capability::Unit(_) => Err(RemoteError::UnknownVariant),
            fsandbox::Capability::Handle(handle) => {
                Ok(Capability::Handle(crate::Handle::new(handle)))
            }
            fsandbox::Capability::Data(data_capability) => Ok(Capability::Data(
                <crate::Data as std::convert::TryFrom<fsandbox::Data>>::try_from(data_capability)?,
            )),
            fsandbox::Capability::Dictionary(dict) => {
                let any = try_from_handle_in_registry(dict.token.as_handle_ref())?;
                match &any {
                    Capability::Dictionary(_) => (),
                    _ => return Err(RemoteError::BadCapability),
                };
                Ok(any)
            }
            fsandbox::Capability::Connector(connector) => {
                let any = try_from_handle_in_registry(connector.token.as_handle_ref())?;
                match &any {
                    Capability::Connector(_) => (),
                    _ => return Err(RemoteError::BadCapability),
                };
                Ok(any)
            }
            fsandbox::Capability::DirConnector(connector) => {
                let any = try_from_handle_in_registry(connector.token.as_handle_ref())?;
                match &any {
                    Capability::DirConnector(_) => (),
                    _ => return Err(RemoteError::BadCapability),
                };
                Ok(any)
            }
            fsandbox::Capability::ConnectorRouter(client_end) => {
                let any = try_from_handle_in_registry(client_end.as_handle_ref())?;
                match &any {
                    Capability::ConnectorRouter(_) => (),
                    _ => return Err(RemoteError::BadCapability),
                };
                Ok(any)
            }
            fsandbox::Capability::DictionaryRouter(client_end) => {
                let any = try_from_handle_in_registry(client_end.as_handle_ref())?;
                match &any {
                    Capability::DictionaryRouter(_) => (),
                    _ => return Err(RemoteError::BadCapability),
                };
                Ok(any)
            }
            fsandbox::Capability::DirEntryRouter(_) => Err(RemoteError::UnknownVariant),
            fsandbox::Capability::DataRouter(client_end) => {
                let any = try_from_handle_in_registry(client_end.as_handle_ref())?;
                match &any {
                    Capability::DataRouter(_) => (),
                    _ => return Err(RemoteError::BadCapability),
                };
                Ok(any)
            }
            fsandbox::Capability::DirEntry(_) => Err(RemoteError::UnknownVariant),
            fsandbox::CapabilityUnknown!() => Err(RemoteError::UnknownVariant),
        }
    }
}

impl Capability {
    pub fn try_into_directory_entry(
        self,
        scope: ExecutionScope,
        token: Arc<WeakInstanceToken>,
    ) -> Result<Arc<dyn DirectoryEntry>, ConversionError> {
        match self {
            Self::Connector(s) => s.try_into_directory_entry(scope, token),
            Self::DirConnector(s) => s.try_into_directory_entry(scope, token),
            Self::ConnectorRouter(s) => s.try_into_directory_entry(scope, token),
            Self::DictionaryRouter(s) => s.try_into_directory_entry(scope, token),
            Self::DirConnectorRouter(s) => s.try_into_directory_entry(scope, token),
            Self::DataRouter(s) => s.try_into_directory_entry(scope, token),
            Self::Dictionary(s) => s.try_into_directory_entry(scope, token),
            Self::Data(s) => Arc::new(s).try_into_directory_entry(scope, token),
            Self::Handle(s) => s.try_into_directory_entry(scope, token),
            Self::Instance(s) => s.try_into_directory_entry(scope, token),
        }
    }
}
