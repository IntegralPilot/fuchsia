// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CapabilityBound, Router, WeakInstanceToken};
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use router_error::{Explain, RouterError};
use std::sync::Arc;
use vfs::directory::entry::{self, DirectoryEntry, DirectoryEntryAsync, EntryInfo, GetEntryInfo};
use vfs::execution_scope::ExecutionScope;
use zx;

/// Binds a Route request from fidl to the Rust [Router::Route] API. Shared by
/// [Router] server implementations.
pub(crate) async fn route_from_fidl<T>(
    router: &Router<T>,
    payload: fsandbox::RouteRequest,
    token: Arc<WeakInstanceToken>,
) -> Result<Option<Arc<T>>, fsandbox::RouterError>
where
    T: CapabilityBound,
{
    let resp = match payload.requesting {
        Some(token) => {
            let capability =
                crate::fidl::registry::get(token.token.as_handle_ref().koid().unwrap());
            let component = match capability {
                Some(crate::Capability::Instance(c)) => c,
                Some(_) => return Err(fsandbox::RouterError::InvalidArgs),
                None => return Err(fsandbox::RouterError::InvalidArgs),
            };
            router.route(RouteRequest::default(), component).await?
        }
        None => router.route(RouteRequest::default(), token).await?,
    };
    Ok(resp)
}

impl<T: CapabilityBound> Router<T> {
    pub(crate) fn into_directory_entry(
        self: Arc<Self>,
        entry_type: fio::DirentType,
        scope: ExecutionScope,
        token: Arc<WeakInstanceToken>,
    ) -> Arc<dyn DirectoryEntry> {
        struct RouterEntry<T: CapabilityBound> {
            router: Arc<Router<T>>,
            entry_type: fio::DirentType,
            scope: ExecutionScope,
            token: Arc<WeakInstanceToken>,
        }

        impl<T: CapabilityBound> DirectoryEntry for RouterEntry<T> {
            fn open_entry(
                self: Arc<Self>,
                mut request: entry::OpenRequest<'_>,
            ) -> Result<(), zx::Status> {
                request.set_scope(self.scope.clone());
                request.spawn(self);
                Ok(())
            }
        }

        impl<T: CapabilityBound> GetEntryInfo for RouterEntry<T> {
            fn entry_info(&self) -> EntryInfo {
                EntryInfo::new(fio::INO_UNKNOWN, self.entry_type)
            }
        }

        impl<T: CapabilityBound> DirectoryEntryAsync for RouterEntry<T> {
            async fn open_entry_async(
                self: Arc<Self>,
                open_request: entry::OpenRequest<'_>,
            ) -> Result<(), zx::Status> {
                // Hold a guard to prevent this task from being dropped during component
                // destruction.  This task is tied to the target component.
                let Some(_guard) = open_request.scope().try_active_guard() else {
                    return Err(zx::Status::PEER_CLOSED);
                };

                // Request a capability from the `router`.
                let result =
                    match self.router.route(RouteRequest::default(), self.token.clone()).await {
                        Ok(Some(c)) => Ok(c),
                        Ok(None) => {
                            return Err(zx::Status::NOT_FOUND);
                        }
                        Err(e) => Err(e),
                    };
                let error = match result {
                    Ok(capability) => {
                        match capability
                            .try_into_directory_entry(self.scope.clone(), self.token.clone())
                        {
                            Ok(open) => return open.open_entry(open_request),
                            Err(_) => RouterError::NotSupported,
                        }
                    }
                    Err(error) => error, // Routing failed (e.g. broken route).
                };
                Err(error.as_zx_status())
            }
        }

        Arc::new(RouterEntry { router: self, entry_type, scope, token })
    }
}
