// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Convenience functions for accessing directories of a component instance
//! and opening protocols that exist in them.

use flex_client::fidl::ProtocolMarker;
use flex_client::ProxyHasDomain;
use moniker::Moniker;
use thiserror::Error;
use {flex_fuchsia_io as fio, flex_fuchsia_sys2 as fsys};

/// Errors that can be returned from opening a component instance directory.
#[derive(Debug, Error)]
pub enum OpenError {
    #[error("instance {0} could not be found")]
    InstanceNotFound(Moniker),
    #[error("opening {0} requires {1} to be resolved")]
    InstanceNotResolved(OpenDirType, Moniker),
    #[error("opening {0} requires {1} to be running")]
    InstanceNotRunning(OpenDirType, Moniker),
    #[error("component manager's open request on the directory returned a FIDL error")]
    OpenDirectoryFidlError,
    #[error("{0} does not have a {1}")]
    NoSuchDir(Moniker, OpenDirType),
    #[error("component manager could not parse moniker: {0}")]
    BadMoniker(Moniker),
    #[error("component manager could not parse dir type: {0}")]
    BadDirType(OpenDirType),
    #[error("component manager could not parse path: {0}")]
    BadPath(String),
    #[error("component manager responded with an unknown error code")]
    UnknownError,
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
}

/// The directories of a component instance that can be opened.
#[derive(Clone, Debug)]
pub enum OpenDirType {
    /// Served by the component's program. Rights unknown.
    Outgoing,
    /// Served by the component's runner. Rights unknown.
    Runtime,
    /// Served by the component's resolver. Rights unknown.
    Package,
    /// Served by component manager. Directory has RW rights.
    Exposed,
    /// Served by component manager. Directory has RW rights.
    Namespace,
}

impl std::fmt::Display for OpenDirType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Outgoing => write!(f, "outgoing directory"),
            Self::Runtime => write!(f, "runtime directory"),
            Self::Package => write!(f, "package directory"),
            Self::Exposed => write!(f, "exposed directory"),
            Self::Namespace => write!(f, "namespace directory"),
        }
    }
}

impl Into<fsys::OpenDirType> for OpenDirType {
    fn into(self) -> fsys::OpenDirType {
        match self {
            Self::Outgoing => fsys::OpenDirType::OutgoingDir,
            Self::Runtime => fsys::OpenDirType::RuntimeDir,
            Self::Package => fsys::OpenDirType::PackageDir,
            Self::Exposed => fsys::OpenDirType::ExposedDir,
            Self::Namespace => fsys::OpenDirType::NamespaceDir,
        }
    }
}

impl From<fsys::OpenDirType> for OpenDirType {
    fn from(fidl_type: fsys::OpenDirType) -> Self {
        match fidl_type {
            fsys::OpenDirType::OutgoingDir => Self::Outgoing,
            fsys::OpenDirType::RuntimeDir => Self::Runtime,
            fsys::OpenDirType::PackageDir => Self::Package,
            fsys::OpenDirType::ExposedDir => Self::Exposed,
            fsys::OpenDirType::NamespaceDir => Self::Namespace,
            fsys::OpenDirTypeUnknown!() => panic!("This should not be constructed"),
        }
    }
}

/// Opens a protocol in a component instance directory, assuming it is located at the root.
pub async fn connect_to_instance_protocol<P: ProtocolMarker>(
    moniker: &Moniker,
    dir_type: OpenDirType,
    realm: &fsys::RealmQueryProxy,
) -> Result<P::Proxy, OpenError> {
    connect_to_instance_protocol_at_path::<P>(moniker, dir_type, P::DEBUG_NAME, realm).await
}

/// Opens a protocol in a component instance directory at the given |path|.
pub async fn connect_to_instance_protocol_at_path<P: ProtocolMarker>(
    moniker: &Moniker,
    dir_type: OpenDirType,
    path: &str,
    realm: &fsys::RealmQueryProxy,
) -> Result<P::Proxy, OpenError> {
    let dir_client = open_instance_directory(moniker, dir_type, realm).await?;
    let (proxy, server_end) = realm.domain().create_proxy::<P>();
    dir_client
        .open(path, fio::Flags::PROTOCOL_SERVICE, &Default::default(), server_end.into_channel())
        .map_err(|_| OpenError::OpenDirectoryFidlError)?;
    Ok(proxy)
}

/// Opens the specified directory type in a component instance identified by `moniker`.
pub async fn open_instance_directory(
    moniker: &Moniker,
    dir_type: OpenDirType,
    realm: &fsys::RealmQueryProxy,
) -> Result<fio::DirectoryProxy, OpenError> {
    let moniker_str = moniker.to_string();
    let (dir_client, dir_server) = realm.domain().create_proxy::<fio::DirectoryMarker>();
    realm
        .open_directory(&moniker_str, dir_type.clone().into(), dir_server)
        .await
        .map_err(|e| OpenError::Fidl(e))?
        .map_err(|e| match e {
            fsys::OpenError::InstanceNotFound => OpenError::InstanceNotFound(moniker.clone()),
            fsys::OpenError::InstanceNotResolved => {
                OpenError::InstanceNotResolved(dir_type, moniker.clone())
            }
            fsys::OpenError::InstanceNotRunning => {
                OpenError::InstanceNotRunning(dir_type, moniker.clone())
            }
            fsys::OpenError::NoSuchDir => OpenError::NoSuchDir(moniker.clone(), dir_type),
            fsys::OpenError::BadDirType => OpenError::BadDirType(dir_type),
            fsys::OpenError::BadMoniker => OpenError::BadMoniker(moniker.clone()),
            fsys::OpenError::FidlError => OpenError::OpenDirectoryFidlError,
            _ => OpenError::UnknownError,
        })?;
    Ok(dir_client)
}
