// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{ClientEnd, Proxy as _};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use log::debug;
use std::collections::HashSet;
use thiserror::Error;
use {
    fidl_fuchsia_component_runner as frunner, fidl_fuchsia_io as fio,
    fidl_fuchsia_process as fprocess, fidl_fuchsia_sys2 as fsys,
};

use crate::error::MissingFidlFieldError;
use crate::util::{self, ConnectToProtocolError};

pub struct Namespace {
    pub overrides: Vec<fprocess::NameInfo>,
    pub directories_fixup: bool,
    pub pkg: Option<ClientEnd<fio::DirectoryMarker>>,
}

impl Namespace {
    pub async fn build(self) -> Result<Vec<fprocess::NameInfo>, NamespaceError> {
        let Self { overrides, directories_fixup, pkg } = self;
        let query = util::connect_to_protocol::<fsys::RealmQueryMarker>()?;
        let console_namespace = query.construct_namespace("/").await??;
        let mut overridden_paths = HashSet::new();
        for fprocess::NameInfo { path, .. } in overrides.iter() {
            // Only accept valid paths.
            if !path.starts_with("/") {
                return Err(NamespaceError::InvalidPath(path.clone()));
            }

            // TODO: We should have better checking here of non-clobbering.
            if !overridden_paths.insert(path.clone()) {
                return Err(NamespaceError::DuplicatePath(path.clone()));
            }
        }
        let mut name_info = overrides;

        let mut push = |item: fprocess::NameInfo| {
            // TODO: We should have better checking here of non-clobbering.
            if !overridden_paths.contains(&item.path) {
                name_info.push(item);
            }
        };

        for frunner::ComponentNamespaceEntry { path, directory, .. } in console_namespace {
            let path = path.ok_or(MissingFidlFieldError("ComponentNamespaceEntry.path"))?;
            let directory =
                directory.ok_or(MissingFidlFieldError("ComponentNamespaceEntry.directory"))?;

            match path.as_str() {
                "/pkg" => {
                    // Skip the /pkg entry, the toolbox /pkg is never something
                    // we want to look at.
                }
                "/directories" if directories_fixup => {
                    let directory = directory.into_proxy();
                    let mut name_infos_stream =
                        std::pin::pin!(replace_directories(&directory).await?);
                    while let Some(info) = name_infos_stream.try_next().await? {
                        push(info);
                    }
                }
                _ => push(fprocess::NameInfo { path, directory }),
            }
        }

        if let Some(directory) = pkg {
            push(fprocess::NameInfo { path: "/pkg".to_string(), directory });
        }

        Ok(name_info)
    }
}

async fn replace_directories(
    parent_dir: &fio::DirectoryProxy,
) -> Result<impl Stream<Item = Result<fprocess::NameInfo, NamespaceError>> + '_, NamespaceError> {
    let watcher = fuchsia_fs::directory::Watcher::new(parent_dir).await?;
    let stream = watcher
        .map_err(NamespaceError::from)
        .scan((), |(), event| async move {
            let fuchsia_fs::directory::WatchMessage { event, filename } = match event {
                Ok(e) => e,
                Err(e) => return Some(Err(e)),
            };
            match event {
                fuchsia_fs::directory::WatchEvent::EXISTING => Some(Ok(Some(filename))),
                fuchsia_fs::directory::WatchEvent::IDLE => None,
                // This is not an enum we can't match exhaustively here. In any
                // case we only care about existing directories.
                _ => Some(Ok(None)),
            }
        })
        .try_filter_map(move |entry| async move {
            let Some(entry) = entry else {
                return Ok(None);
            };
            // Ignore the self entry.
            if entry.to_str().is_some_and(|entry| entry == ".") {
                return Ok(None);
            }

            Ok(Some(async move {
                let path = entry
                    .to_str()
                    .ok_or(NamespaceError::InvalidPath(entry.to_string_lossy().to_string()))?;
                let directory = fuchsia_fs::directory::open_directory(
                    parent_dir,
                    path,
                    fio::PERM_READABLE
                        | fio::Flags::PERM_INHERIT_EXECUTE
                        | fio::Flags::PERM_INHERIT_WRITE,
                )
                .await;

                let directory = match directory {
                    Ok(directory) => directory,
                    Err(e @ fuchsia_fs::node::OpenError::OnOpenEventStreamClosed)
                    | Err(
                        e @ fuchsia_fs::node::OpenError::OpenError(
                            zx::Status::NOT_FOUND | zx::Status::PEER_CLOSED,
                        ),
                    ) => {
                        // Some directories may fail to open, because they're
                        // optional or not served.
                        //
                        // This happens with `build-info` in some
                        // configurations and offers from `void`.
                        // We avoid logging this loudly to prevent log spam.
                        debug!("failed to open directory '{path}': {e}. Skipping.");
                        return Ok(None);
                    }
                    Err(e) => {
                        return Err(NamespaceError::DirectoryOpen {
                            name: path.to_string(),
                            error: e,
                        });
                    }
                };

                let directory = ClientEnd::<fio::DirectoryMarker>::new(
                    directory.into_channel().unwrap().into(),
                );

                // Apply specific rules to directory entries.
                //
                // We do this so we can provide the same namespace layout for
                // shell that systems in Fuchsia have historically used. The SSH
                // realm and the serial console realm both use the same mappings
                // here and developer console aims to be a full drop-in
                // replacement.
                //
                // These are documented in the FIDL API, so try to keep that in
                // sync.
                // LINT.IfChange
                let path = match path {
                    "root-ssl-certificates" => "/config/ssl".to_string(),
                    "build-info" => "/config/build-info".to_string(),
                    path => format!("/{path}"),
                };
                // LINT.ThenChange(//sdk/fidl/fuchsia.developer.console/launcher.fidl)

                Ok(Some(fprocess::NameInfo { path, directory }))
            }))
        })
        // Let some of the directory opening happen in parallel.
        .try_buffer_unordered(16)
        .try_filter_map(futures::future::ok);
    Ok(stream)
}

#[derive(Error, Debug)]
pub enum NamespaceError {
    #[error(transparent)]
    Proto(#[from] ConnectToProtocolError),
    #[error("unexpected FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("failed to construct namespace: {0:?}")]
    ConstructNamespace(fsys::ConstructNamespaceError),
    #[error("invalid path \"{0}\"")]
    InvalidPath(String),
    #[error("duplicate path \"{0}\"")]
    DuplicatePath(String),
    #[error("failed to create directory watcher: {0}")]
    DirectoryWatcherCreate(#[from] fuchsia_fs::directory::WatcherCreateError),
    #[error("failed to watch directory: {0}")]
    DirectoryWatcherStream(#[from] fuchsia_fs::directory::WatcherStreamError),
    #[error("failed to open directory '{name}': {error}")]
    DirectoryOpen { name: String, error: fuchsia_fs::node::OpenError },
    #[error(transparent)]
    MissingFidlField(#[from] MissingFidlFieldError),
}

impl From<fsys::ConstructNamespaceError> for NamespaceError {
    fn from(value: fsys::ConstructNamespaceError) -> Self {
        Self::ConstructNamespace(value)
    }
}
