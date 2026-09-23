// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utility functions for fuchsia.io nodes.

use flex_fuchsia_io as fio;
use futures::prelude::*;
use thiserror::Error;

#[cfg(target_os = "fuchsia")]
#[cfg(not(feature = "fdomain"))]
pub use fuchsia::*;

#[cfg(target_os = "fuchsia")]
#[cfg(not(feature = "fdomain"))]
mod fuchsia {
    use super::*;

    /// Opens the given `path` from the current namespace as a [`NodeProxy`].
    ///
    /// The target is assumed to implement fuchsia.io.Node but this isn't verified. To connect to a
    /// filesystem node which doesn't implement fuchsia.io.Node, use the functions in
    /// [`fuchsia_component::client`] instead.
    ///
    /// If the namespace path doesn't exist, or we fail to make the channel pair, this returns an
    /// error. However, if incorrect flags are sent, or if the rest of the path sent to the
    /// filesystem server doesn't exist, this will still return success. Instead, the returned
    /// NodeProxy channel pair will be closed with an epitaph.
    pub fn open_in_namespace(path: &str, flags: fio::Flags) -> Result<fio::NodeProxy, OpenError> {
        let (node, request) = fidl::endpoints::create_proxy();
        open_channel_in_namespace(path, flags, request)?;
        Ok(node)
    }

    /// Asynchronously opens the given [`path`] in the current namespace, serving the connection
    /// over [`request`]. Once the channel is connected, any calls made prior are serviced.
    ///
    /// The target is assumed to implement fuchsia.io.Node but this isn't verified. To connect to a
    /// filesystem node which doesn't implement fuchsia.io.Node, use the functions in
    /// [`fuchsia_component::client`] instead.
    ///
    /// If the namespace path doesn't exist, this returns an error. However, if incorrect flags are
    /// sent, or if the rest of the path sent to the filesystem server doesn't exist, this will
    /// still return success. Instead, the [`request`] channel will be closed with an epitaph.
    pub fn open_channel_in_namespace(
        path: &str,
        flags: fio::Flags,
        request: fidl::endpoints::ServerEnd<fio::NodeMarker>,
    ) -> Result<(), OpenError> {
        let namespace = fdio::Namespace::installed().map_err(OpenError::Namespace)?;
        namespace.open(path, flags, request.into_channel()).map_err(OpenError::Namespace)
    }
}

/// An error encountered while opening a node
#[derive(Debug, Clone, Error)]
#[allow(missing_docs)]
pub enum OpenError {
    #[error("while making a fidl proxy: {0}")]
    CreateProxy(#[source] flex_client::Error),

    #[error("while opening from namespace: {0}")]
    Namespace(#[source] zx_status::Status),

    #[error("while sending open request: {0}")]
    SendOpenRequest(#[source] fidl::Error),

    #[error("node event stream closed prematurely")]
    OnOpenEventStreamClosed,

    #[error("while reading OnOpen event: {0}")]
    OnOpenDecode(#[source] fidl::Error),

    #[error("open failed with status: {0}")]
    OpenError(#[source] zx_status::Status),

    #[error("remote responded with success but provided no node info")]
    MissingOnOpenInfo,

    #[error("expected node to be a {expected:?}, but got a {actual:?}")]
    UnexpectedNodeKind { expected: Kind, actual: Kind },

    #[error("received unknown event (ordinal = {ordinal})")]
    UnknownEvent { ordinal: u64 },
}

impl OpenError {
    /// Returns true if the open failed because the node was not found.
    pub fn is_not_found_error(&self) -> bool {
        matches!(
            self,
            OpenError::OpenError(zx_status::Status::NOT_FOUND)
                | OpenError::Namespace(zx_status::Status::NOT_FOUND)
        )
    }
}

/// An error encountered while cloning a node
#[derive(Debug, Clone, Error)]
#[allow(missing_docs)]
pub enum CloneError {
    #[error("while making a fidl proxy: {0}")]
    CreateProxy(#[source] fidl::Error),

    #[error("while sending clone request: {0}")]
    SendCloneRequest(#[source] fidl::Error),
}

/// An error encountered while closing a node
#[derive(Debug, Clone, Error)]
#[allow(missing_docs)]
pub enum CloseError {
    #[error("while sending close request: {0}")]
    SendCloseRequest(#[source] fidl::Error),

    #[error("close failed with status: {0}")]
    CloseError(#[source] zx_status::Status),
}

/// An error encountered while renaming a node
#[derive(Debug, Clone, Error)]
#[allow(missing_docs)]
pub enum RenameError {
    #[error("while sending rename request")]
    SendRenameRequest(#[source] fidl::Error),

    #[error("while sending get_token request")]
    SendGetTokenRequest(#[source] fidl::Error),

    #[error("rename failed with status")]
    RenameError(#[source] zx_status::Status),

    #[error("while opening subdirectory")]
    OpenError(#[from] OpenError),

    #[error("get_token failed with status")]
    GetTokenError(#[source] zx_status::Status),

    #[error("no handle from get token")]
    NoHandleError,
}

/// The type of a filesystem node
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Kind {
    Service,
    File,
    Directory,
    Symlink,
    Unknown,
}

impl Kind {
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    pub(crate) fn kind_of(info: &fio::NodeInfoDeprecated) -> Kind {
        match info {
            fio::NodeInfoDeprecated::Service(_) => Kind::Service,
            fio::NodeInfoDeprecated::File(_) => Kind::File,
            fio::NodeInfoDeprecated::Directory(_) => Kind::Directory,
            fio::NodeInfoDeprecated::Symlink(_) => Kind::Symlink,
        }
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn expect_file(info: fio::NodeInfoDeprecated) -> Result<(), Kind> {
        match info {
            fio::NodeInfoDeprecated::File(fio::FileObject { .. }) => Ok(()),
            other => Err(Kind::kind_of(&other)),
        }
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn expect_directory(info: fio::NodeInfoDeprecated) -> Result<(), Kind> {
        match info {
            fio::NodeInfoDeprecated::Directory(fio::DirectoryObject) => Ok(()),
            other => Err(Kind::kind_of(&other)),
        }
    }

    pub(crate) fn kind_of2(representation: &fio::Representation) -> Kind {
        match representation {
            fio::Representation::Directory(_) => Kind::Directory,
            fio::Representation::File(_) => Kind::File,
            fio::Representation::Symlink(_) => Kind::Symlink,
            _ => Kind::Unknown,
        }
    }

    fn expect_file2(representation: &fio::Representation) -> Result<(), Kind> {
        match representation {
            fio::Representation::File(fio::FileInfo { .. }) => Ok(()),
            other => Err(Kind::kind_of2(other)),
        }
    }

    fn expect_directory2(representation: &fio::Representation) -> Result<(), Kind> {
        match representation {
            fio::Representation::Directory(_) => Ok(()),
            other => Err(Kind::kind_of2(other)),
        }
    }
}

/// Gracefully closes the node proxy from the remote end.
pub async fn close(node: fio::NodeProxy) -> Result<(), CloseError> {
    let result = node.close().await.map_err(CloseError::SendCloseRequest)?;
    result.map_err(|s| CloseError::CloseError(zx_status::Status::err_from_raw(s)))
}

/// Consume the first event from this NodeProxy's event stream, returning the proxy if it is
/// the expected type or an error otherwise.
pub(crate) async fn verify_node_describe_event(
    node: fio::NodeProxy,
) -> Result<fio::NodeProxy, OpenError> {
    match take_on_open_event(&node).await? {
        #[cfg(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        ))]
        fio::NodeEvent::OnOpen_ { s: status, info } => {
            let () = zx_status::Status::ok(status).map_err(OpenError::OpenError)?;
            info.ok_or(OpenError::MissingOnOpenInfo)?;
        }
        fio::NodeEvent::OnRepresentation { .. } => {}
        fio::NodeEvent::_UnknownEvent { ordinal, .. } => {
            return Err(OpenError::UnknownEvent { ordinal });
        }
    }

    Ok(node)
}

/// Consume the first event from this DirectoryProxy's event stream, returning the proxy if it is
/// the expected type or an error otherwise.
pub(crate) async fn verify_directory_describe_event(
    node: fio::DirectoryProxy,
) -> Result<fio::DirectoryProxy, OpenError> {
    match take_on_open_event(&node).await? {
        #[cfg(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        ))]
        fio::DirectoryEvent::OnOpen_ { s: status, info } => {
            let () = zx_status::Status::ok(status).map_err(OpenError::OpenError)?;
            let info = info.ok_or(OpenError::MissingOnOpenInfo)?;
            let () = Kind::expect_directory(*info).map_err(|actual| {
                OpenError::UnexpectedNodeKind { expected: Kind::Directory, actual }
            })?;
        }
        fio::DirectoryEvent::OnRepresentation { payload } => {
            let () = Kind::expect_directory2(&payload).map_err(|actual| {
                OpenError::UnexpectedNodeKind { expected: Kind::Directory, actual }
            })?;
        }
        fio::DirectoryEvent::_UnknownEvent { ordinal, .. } => {
            return Err(OpenError::UnknownEvent { ordinal });
        }
    }

    Ok(node)
}

/// Consume the first event from this FileProxy's event stream, returning the proxy if it is the
/// expected type or an error otherwise.
pub(crate) async fn verify_file_describe_event(
    node: fio::FileProxy,
) -> Result<fio::FileProxy, OpenError> {
    match take_on_open_event(&node).await? {
        #[cfg(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        ))]
        fio::FileEvent::OnOpen_ { s: status, info } => {
            let () = zx_status::Status::ok(status).map_err(OpenError::OpenError)?;
            let info = info.ok_or(OpenError::MissingOnOpenInfo)?;
            let () = Kind::expect_file(*info)
                .map_err(|actual| OpenError::UnexpectedNodeKind { expected: Kind::File, actual })?;
        }
        fio::FileEvent::OnRepresentation { payload } => {
            let () = Kind::expect_file2(&payload)
                .map_err(|actual| OpenError::UnexpectedNodeKind { expected: Kind::File, actual })?;
        }
        fio::FileEvent::_UnknownEvent { ordinal, .. } => {
            return Err(OpenError::UnknownEvent { ordinal });
        }
    }

    Ok(node)
}

pub(crate) trait OnOpenEventProducer {
    type Event;
    type Stream: futures::Stream<Item = Result<Self::Event, fidl::Error>> + Unpin;
    fn take_event_stream(&self) -> Self::Stream;
}

macro_rules! impl_on_open_event_producer {
    ($proxy:ty, $event:ty, $stream:ty) => {
        impl OnOpenEventProducer for $proxy {
            type Event = $event;
            type Stream = $stream;
            fn take_event_stream(&self) -> Self::Stream {
                self.take_event_stream()
            }
        }
    };
}

impl_on_open_event_producer!(fio::NodeProxy, fio::NodeEvent, fio::NodeEventStream);
impl_on_open_event_producer!(fio::FileProxy, fio::FileEvent, fio::FileEventStream);
impl_on_open_event_producer!(fio::DirectoryProxy, fio::DirectoryEvent, fio::DirectoryEventStream);

pub(crate) async fn take_on_open_event<T>(node: &T) -> Result<T::Event, OpenError>
where
    T: OnOpenEventProducer,
{
    node.take_event_stream().next().await.ok_or(OpenError::OnOpenEventStreamClosed)?.map_err(|e| {
        if let fidl::Error::ClientChannelClosed { epitaph, .. } = e {
            OpenError::OpenError(match epitaph.into() {
                Err(s) => s,
                Ok(()) => zx_status::Status::PEER_CLOSED,
            })
        } else {
            OpenError::OnOpenDecode(e)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::DiscoverableProtocolMarker as _;
    use fuchsia_async as fasync;

    // open_in_namespace

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_opens_real_node() {
        let file_node = open_in_namespace("/pkg/data/file", fio::PERM_READABLE).unwrap();
        let protocol = file_node.query().await.unwrap();
        assert_eq!(protocol, fio::FileMarker::PROTOCOL_NAME.as_bytes());

        let dir_node = open_in_namespace("/pkg/data", fio::PERM_READABLE).unwrap();
        let protocol = dir_node.query().await.unwrap();
        assert_eq!(protocol, fio::DirectoryMarker::PROTOCOL_NAME.as_bytes());
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_opens_fake_node_under_of_root_namespace_entry() {
        let notfound = open_in_namespace("/pkg/fake", fio::PERM_READABLE).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(close(notfound).await, Err(_));
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_rejects_fake_root_namespace_entry() {
        assert_matches!(
            open_in_namespace("/fake", fio::PERM_READABLE),
            Err(OpenError::Namespace(zx_status::Status::NOT_FOUND))
        );
    }
}
