// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CapabilityBound, DirReceiver};
use cm_types::RelativePath;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use futures::channel::mpsc;
use std::fmt::Debug;
use std::sync::{Arc, LazyLock};

/// These are the flags which may always be set when opening something through a DirConnector. See
/// the comment on [`DirConnectable::maximum_flags`] for more information.
static ALWAYS_ALLOWED_FLAGS: LazyLock<fio::Flags> = LazyLock::new(|| {
    fio::Flags::PROTOCOL_SERVICE
        | fio::Flags::PROTOCOL_NODE
        | fio::Flags::PROTOCOL_DIRECTORY
        | fio::Flags::PROTOCOL_FILE
        | fio::Flags::PROTOCOL_SYMLINK
        | fio::Flags::FLAG_SEND_REPRESENTATION
        | fio::Flags::FLAG_MAYBE_CREATE
        | fio::Flags::FLAG_MUST_CREATE
        | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
        | fio::Flags::FILE_APPEND
        | fio::Flags::FILE_TRUNCATE
});

/// Types that implement [`DirConnectable`] let the holder send directory channels
/// to them. Any `DirConnectable` should be wrapped in a [`DirConnector`].
pub trait DirConnectable: Send + Sync + Debug {
    /// Returns the maximum set of flags that may be passed to this DirConnectable. For example, to
    /// disallow calling `send` with write permissions, this function could return
    /// `fidl_fuchsia_io::PERM_READABLE`.
    ///
    /// The following flags are always permitted, regardless of the returned value:
    ///
    /// - `fidl_fuchsia_io::Flags::PROTOCOL_SERVICE`
    /// - `fidl_fuchsia_io::Flags::PROTOCOL_NODE`
    /// - `fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY`
    /// - `fidl_fuchsia_io::Flags::PROTOCOL_FILE`
    /// - `fidl_fuchsia_io::Flags::PROTOCOL_SYMLINK`
    /// - `fidl_fuchsia_io::Flags::FLAG_SEND_REPRESENTATION`
    /// - `fidl_fuchsia_io::Flags::FLAG_MAYBE_CREATE`
    /// - `fidl_fuchsia_io::Flags::FLAG_MUST_CREATE`
    /// - `fidl_fuchsia_io::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY`
    /// - `fidl_fuchsia_io::Flags::FILE_APPEND`
    /// - `fidl_fuchsia_io::Flags::FILE_TRUNCATE`
    ///
    /// If the returned value does not contain the set of flags
    /// `fidl_fuchsia_io::INHERITED_WRITE_PERMISSIONS`, then
    /// `fidl_fuchsia_io::Flags::PERM_INHERIT_WRITE` will be stripped from any flags passed to
    /// `send` if it is set.
    ///
    /// If the returned value does not contain the flag
    /// `fidl_fuchsia_io::Flags::PERM_INHERIT_EXECUTE`, then `fidl_fuchsia_io::Flags::PERM_EXECUTE`
    /// will be stripped from any flags passed to `send` if it is set.
    fn maximum_flags(&self) -> fio::Flags;

    fn send(
        &self,
        dir: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        flags: Option<fio::Flags>,
    ) -> Result<(), ()>;
}

pub struct DirConnectorMessage {
    pub dir: ServerEnd<fio::DirectoryMarker>,
    pub subdir: RelativePath,
    pub flags: Option<fio::Flags>,
}

impl DirConnectable for mpsc::UnboundedSender<DirConnectorMessage> {
    /// An unbounded sender receiver channel does not restrict permissions on incoming directory
    /// connections. Returning full permissions allows callers (including VFS lookups that pass
    /// `PERM_READABLE`/`TRAVERSE`) to send directory requests without failing `DirConnector::send`
    /// permission validation.
    fn maximum_flags(&self) -> fio::Flags {
        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE
    }

    fn send(
        &self,
        dir: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        flags: Option<fio::Flags>,
    ) -> Result<(), ()> {
        self.unbounded_send(DirConnectorMessage { dir, subdir, flags }).map_err(|_| ())
    }
}

/// A capability to obtain a channel to a [fuchsia.io/Directory]. As the name suggests, this is
/// similar to [Connector], except the channel type is always [fuchsia.io/Directory], and vfs
/// nodes that wrap this capability should have the `DIRECTORY` entry_type.
#[derive(Debug)]
pub struct DirConnector {
    inner: Box<dyn DirConnectable>,
}

impl CapabilityBound for DirConnector {
    fn debug_typename() -> &'static str {
        "DirConnector"
    }

    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        _scope: vfs::execution_scope::ExecutionScope,
        _token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Ok(Arc::new(crate::fidl::dir_connector::DirConnectorDirectoryEntry { dir_connector: self }))
    }
}

impl DirConnector {
    pub fn new() -> (DirReceiver, Arc<Self>) {
        let (sender, receiver) = mpsc::unbounded();
        let receiver = DirReceiver::new(receiver);
        let this = Self::new_sendable(sender);
        (receiver, this)
    }

    pub fn from_proxy(
        proxy: fio::DirectoryProxy,
        subdir: RelativePath,
        flags: fio::Flags,
    ) -> Arc<Self> {
        Self::new_sendable(DirectoryProxyForwarder { proxy, subdir, flags })
    }

    pub fn new_sendable(connector: impl DirConnectable + 'static) -> Arc<Self> {
        Arc::new(Self { inner: Box::new(connector) })
    }

    pub fn send(
        &self,
        dir: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        mut flags: Option<fio::Flags>,
    ) -> Result<(), ()> {
        if let Some(flags) = flags.as_mut() {
            let mut maximum_flags_and_always_allowed =
                self.inner.maximum_flags() | *ALWAYS_ALLOWED_FLAGS;
            if flags.contains(fio::Flags::PERM_INHERIT_WRITE) {
                if !maximum_flags_and_always_allowed.contains(
                    fio::Flags::from_bits(fio::INHERITED_WRITE_PERMISSIONS.bits()).unwrap(),
                ) {
                    flags.remove(fio::Flags::PERM_INHERIT_WRITE);
                } else {
                    maximum_flags_and_always_allowed.insert(fio::Flags::PERM_INHERIT_WRITE);
                }
            }
            if flags.contains(fio::Flags::PERM_INHERIT_EXECUTE) {
                if !maximum_flags_and_always_allowed.contains(fio::Flags::PERM_EXECUTE) {
                    flags.remove(fio::Flags::PERM_INHERIT_EXECUTE);
                } else {
                    maximum_flags_and_always_allowed.insert(fio::Flags::PERM_INHERIT_EXECUTE);
                }
            }
            if !maximum_flags_and_always_allowed.contains(*flags) {
                // The caller has requested greater permissions than is allowed.
                return Err(());
            }
        }
        self.inner.send(dir, subdir, flags)
    }

    pub fn with_subdir(self: Arc<Self>, subdir: RelativePath) -> Arc<Self> {
        Self::new_sendable(DirConnectorSubdir { parent_dir_connector: self, subdir })
    }
}

impl DirConnectable for DirConnector {
    fn maximum_flags(&self) -> fio::Flags {
        self.inner.maximum_flags()
    }

    fn send(
        &self,
        channel: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        flags: Option<fio::Flags>,
    ) -> Result<(), ()> {
        self.inner.send(channel, subdir, flags)
    }
}

#[derive(Debug)]
struct DirConnectorSubdir {
    parent_dir_connector: Arc<DirConnector>,
    subdir: RelativePath,
}

impl DirConnectable for DirConnectorSubdir {
    fn maximum_flags(&self) -> fio::Flags {
        self.parent_dir_connector.maximum_flags()
    }

    fn send(
        &self,
        channel: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        flags: Option<fio::Flags>,
    ) -> Result<(), ()> {
        let mut combined_subdir = self.subdir.clone();
        let success = combined_subdir.extend(subdir);
        if !success {
            // subdir is too long
            return Err(());
        }
        self.parent_dir_connector.send(channel, combined_subdir, flags)
    }
}

#[derive(Debug)]
struct DirectoryProxyForwarder {
    proxy: fio::DirectoryProxy,
    subdir: RelativePath,
    flags: fio::Flags,
}

impl DirConnectable for DirectoryProxyForwarder {
    fn maximum_flags(&self) -> fio::Flags {
        self.flags
    }

    fn send(
        &self,
        server_end: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        flags: Option<fio::Flags>,
    ) -> Result<(), ()> {
        let flags = flags.unwrap_or(self.flags | fio::Flags::PROTOCOL_DIRECTORY);
        let mut combined_subdir = self.subdir.clone();
        let success = combined_subdir.extend(subdir);
        if !success {
            // The requested path is too long.
            return Err(());
        }
        self.proxy
            .open(
                &format!("{}", combined_subdir),
                flags,
                &fio::Options::default(),
                server_end.into_channel(),
            )
            .map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints;
    use fidl::handle::Rights;
    use fidl_fuchsia_component_sandbox as fsandbox;
    use futures::StreamExt;

    // NOTE: sending-and-receiving tests are written in `receiver.rs`.

    /// Tests that a DirConnector can be cloned by cloning its FIDL token.
    /// and capabilities sent to the original and clone arrive at the same Receiver.
    #[fuchsia::test]
    async fn fidl_clone() {
        let (receiver, sender) = DirConnector::new();

        // Send a channel through the DirConnector.
        let (_ch1, ch2) = endpoints::create_endpoints::<fio::DirectoryMarker>();
        sender.send(ch2, RelativePath::dot(), None).unwrap();

        // Convert the Sender to a FIDL token.
        let connector: fsandbox::DirConnector = sender.to_fsandbox();

        // Clone the Sender by cloning the token.
        let token_clone = fsandbox::DirConnector {
            token: connector.token.duplicate_handle(Rights::SAME_RIGHTS).unwrap(),
        };
        let connector_clone =
            match crate::Capability::try_from(fsandbox::Capability::DirConnector(token_clone))
                .unwrap()
            {
                crate::Capability::DirConnector(connector) => connector,
                capability @ _ => panic!("wrong type {capability:?}"),
            };

        // Send a channel through the cloned Sender.
        let (_ch1, ch2) = endpoints::create_endpoints::<fio::DirectoryMarker>();
        connector_clone.send(ch2, RelativePath::dot(), None).unwrap();

        // The Receiver should receive two channels, one from each connector.
        for _ in 0..2 {
            let _ch = receiver.receive().await.unwrap();
        }
    }

    #[fuchsia::test]
    async fn flags_check() {
        #[derive(Debug)]
        struct DirConnectableStruct {
            maximum_flags: fio::Flags,
            sender: mpsc::UnboundedSender<Option<fio::Flags>>,
        }
        impl DirConnectable for DirConnectableStruct {
            fn maximum_flags(&self) -> fio::Flags {
                self.maximum_flags
            }
            fn send(
                &self,
                _dir: ServerEnd<fio::DirectoryMarker>,
                _subdir: RelativePath,
                flags: Option<fio::Flags>,
            ) -> Result<(), ()> {
                self.sender.unbounded_send(flags).unwrap();
                Ok(())
            }
        }

        let (sender, mut receiver) = mpsc::unbounded();
        let dc1 = DirConnector::new_sendable(DirConnectableStruct {
            maximum_flags: fio::PERM_READABLE,
            sender: sender.clone(),
        });

        for (input_flags, expected_output_flags) in [
            (None, None),
            (Some(fio::PERM_READABLE), Some(fio::PERM_READABLE)),
            (
                Some(fio::PERM_READABLE | fio::Flags::FLAG_MUST_CREATE),
                Some(fio::PERM_READABLE | fio::Flags::FLAG_MUST_CREATE),
            ),
            (
                Some(fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE),
                Some(fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE),
            ),
            (Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE), Some(fio::PERM_READABLE)),
            (Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_EXECUTE), Some(fio::PERM_READABLE)),
        ] {
            let (_client, server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            assert_eq!(
                Ok(()),
                dc1.send(server, RelativePath::dot(), input_flags),
                "failed to send input {input_flags:?}"
            );
            assert_eq!(expected_output_flags, receiver.next().await.unwrap());
        }

        let dc2 = DirConnector::new_sendable(DirConnectableStruct {
            maximum_flags: fio::PERM_READABLE | fio::PERM_WRITABLE,
            sender: sender.clone(),
        });
        for (input_flags, expected_output_flags) in [
            (Some(fio::PERM_WRITABLE), Some(fio::PERM_WRITABLE)),
            (
                Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE),
                Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE),
            ),
            (Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_EXECUTE), Some(fio::PERM_READABLE)),
        ] {
            let (_client, server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            assert_eq!(
                Ok(()),
                dc2.send(server, RelativePath::dot(), input_flags),
                "failed to send input {input_flags:?}"
            );
            assert_eq!(expected_output_flags, receiver.next().await.unwrap());
        }

        let dc3 = DirConnector::new_sendable(DirConnectableStruct {
            maximum_flags: fio::PERM_READABLE | fio::PERM_EXECUTABLE,
            sender: sender.clone(),
        });
        for (input_flags, expected_output_flags) in [
            (Some(fio::PERM_EXECUTABLE), Some(fio::PERM_EXECUTABLE)),
            (Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE), Some(fio::PERM_READABLE)),
            (
                Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_EXECUTE),
                Some(fio::PERM_READABLE | fio::Flags::PERM_INHERIT_EXECUTE),
            ),
        ] {
            let (_client, server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            assert_eq!(
                Ok(()),
                dc3.send(server, RelativePath::dot(), input_flags),
                "failed to send input {input_flags:?}"
            );
            assert_eq!(expected_output_flags, receiver.next().await.unwrap());
        }

        for (maximum_flags, input_flags) in [
            (fio::PERM_READABLE, fio::PERM_READABLE | fio::PERM_EXECUTABLE),
            (fio::PERM_READABLE | fio::PERM_WRITABLE, fio::PERM_EXECUTABLE),
            (fio::PERM_READABLE | fio::PERM_EXECUTABLE, fio::PERM_WRITABLE),
        ] {
            let dc = DirConnector::new_sendable(DirConnectableStruct {
                maximum_flags,
                sender: sender.clone(),
            });
            let (_client, server) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            assert_eq!(
                Err(()),
                dc.send(server, RelativePath::dot(), Some(input_flags)),
                "unexpectedly succeeded at sending input {input_flags:?}"
            );
        }
    }
}
