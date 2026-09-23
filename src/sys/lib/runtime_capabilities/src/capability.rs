// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Router;
use from_enum::FromEnum;
use router_error::Explain;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use zx_status;

#[derive(Error, Debug, Clone)]
pub enum ConversionError {
    #[error("conversion to type is not supported")]
    NotSupported,
}

/// Errors arising from conversion between Rust and FIDL types.
#[derive(Error, Debug)]
pub enum RemoteError {
    #[error("unknown FIDL variant")]
    UnknownVariant,

    #[error("unregistered capability; only capabilities created by sandbox are allowed")]
    Unregistered,

    #[error("registered capability had the wrong type")]
    BadCapability,
}

impl Explain for RemoteError {
    fn as_zx_status(&self) -> zx_status::Status {
        match self {
            RemoteError::UnknownVariant => zx_status::Status::NOT_SUPPORTED,
            RemoteError::Unregistered => zx_status::Status::INVALID_ARGS,
            RemoteError::BadCapability => zx_status::Status::INVALID_ARGS,
        }
    }
}

#[derive(FromEnum, Debug, Clone)]
pub enum Capability {
    Connector(Arc<crate::Connector>),
    DirConnector(Arc<crate::DirConnector>),
    Dictionary(Arc<crate::Dictionary>),
    Data(crate::Data),
    Handle(Arc<crate::Handle>),
    ConnectorRouter(Arc<crate::Router<crate::Connector>>),
    DictionaryRouter(Arc<crate::Router<crate::Dictionary>>),
    DirConnectorRouter(Arc<crate::Router<crate::DirConnector>>),
    DataRouter(Arc<crate::Router<crate::Data>>),
    Instance(Arc<crate::WeakInstanceToken>),
}

impl Capability {
    pub fn to_dictionary(self) -> Option<Arc<crate::Dictionary>> {
        match self {
            Self::Dictionary(d) => Some(d),
            _ => None,
        }
    }

    pub fn debug_typename(&self) -> &'static str {
        match self {
            Self::Connector(_) => crate::Connector::debug_typename(),
            Self::DirConnector(_) => crate::DirConnector::debug_typename(),
            Self::ConnectorRouter(_) => crate::Router::<crate::Connector>::debug_typename(),
            Self::DictionaryRouter(_) => crate::Router::<crate::Dictionary>::debug_typename(),
            Self::DirConnectorRouter(_) => crate::Router::<crate::DirConnector>::debug_typename(),
            Self::DataRouter(_) => crate::Router::<crate::Data>::debug_typename(),
            Self::Dictionary(_) => crate::Dictionary::debug_typename(),
            Self::Data(_) => crate::Data::debug_typename(),
            Self::Handle(_) => crate::Handle::debug_typename(),
            Self::Instance(_) => "Instance",
        }
    }

    /// Returns information about the capability which is useful to an [`ErrorLoggingRouter`].
    pub fn error_info(&self) -> Option<crate::RouterErrorInfo> {
        match self {
            Self::ConnectorRouter(r) => r.error_info(),
            Self::DictionaryRouter(r) => r.error_info(),
            Self::DirConnectorRouter(r) => r.error_info(),
            Self::DataRouter(r) => r.error_info(),
            _ => None,
        }
    }
}

impl TryFrom<Capability> for Arc<crate::Connector> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::Connector(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<crate::DirConnector> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::DirConnector(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<crate::Dictionary> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::Dictionary(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for crate::Data {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::Data(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<Router<crate::Dictionary>> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::DictionaryRouter(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<Router<crate::DirConnector>> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::DirConnectorRouter(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<Router<crate::Connector>> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::ConnectorRouter(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<Router<crate::Data>> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::DataRouter(c) => Ok(c),
            _ => Err(()),
        }
    }
}

impl TryFrom<Capability> for Arc<crate::WeakInstanceToken> {
    type Error = ();

    fn try_from(c: Capability) -> Result<Self, Self::Error> {
        match c {
            Capability::Instance(i) => Ok(i),
            _ => Err(()),
        }
    }
}

/// Parent trait implemented by all capability types. Useful for defining interfaces that
/// generic over a capability type.
pub trait CapabilityBound: Send + Sync + 'static {
    fn debug_typename() -> &'static str;

    /// Converts this capability into a directory entry.
    ///
    /// This is on the "main" capability trait because this is something supported by (almost)
    /// every capability type, and all capability types are expected to either support this or be
    /// able to report that they cannot (i.e. return an error when this is called).
    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        _scope: vfs::execution_scope::ExecutionScope,
        _token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Err(ConversionError::NotSupported)
    }
}
