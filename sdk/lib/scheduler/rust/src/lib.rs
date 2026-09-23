// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_scheduler::{
    RoleManagerMarker, RoleManagerSetRoleRequest, RoleManagerSynchronousProxy, RoleName, RoleTarget,
};
use fuchsia_component_client::connect_to_protocol_sync;
use fuchsia_sync::RwLock;
use std::sync::Arc;
use zx::{MonotonicInstant, Rights, Status, Thread, Vmar};

static ROLE_MANAGER: RwLock<Option<Arc<RoleManagerSynchronousProxy>>> = RwLock::new(None);

fn connect() -> Result<Arc<RoleManagerSynchronousProxy>, Error> {
    // If the proxy has been connected already, return.
    if let Some(ref proxy) = *ROLE_MANAGER.read() {
        return Ok(Arc::clone(&proxy));
    }

    // Acquire the write lock and make sure no other thread connected to the proxy while we
    // were waiting on the write lock.
    let mut proxy = ROLE_MANAGER.write();
    if let Some(ref proxy) = *proxy {
        return Ok(Arc::clone(&proxy));
    }

    // Connect to the synchronous proxy.
    let p = Arc::new(connect_to_protocol_sync::<RoleManagerMarker>()?);
    *proxy = Some(Arc::clone(&p));
    Ok(p)
}

fn disconnect() {
    // Drop our connection to the proxy by setting it to None.
    let mut proxy = ROLE_MANAGER.write();
    *proxy = None;
}

fn set_role_for_target(target: RoleTarget, role_name: &str) -> Result<(), Error> {
    let role_manager = connect()?;
    let request = RoleManagerSetRoleRequest {
        target: Some(target),
        role: Some(RoleName { role: role_name.to_string() }),
        ..Default::default()
    };
    let _ = role_manager
        .set_role(request, MonotonicInstant::INFINITE)
        .context("fuchsia.scheduler.RoleManager::SetRole failed")
        .and_then(|result| {
            match result {
                Ok(_) => Ok(()),
                Err(status) => {
                    // If the server responded with ZX_ERR_PEER_CLOSED, mark the synchronous proxy as
                    // disconnected so future invocations of this function reconnect to the RoleManager.
                    if status == Status::PEER_CLOSED.into_raw() {
                        disconnect();
                    }
                    Status::ok(status).context(format!(
                        "fuchsia.scheduler.RoleManager::SetRole returned error: {:?}",
                        status
                    ))
                }
            }
        })?;
    Ok(())
}

pub fn set_role_for_thread(thread: &Thread, role_name: &str) -> Result<(), Error> {
    let thread = thread
        .duplicate_handle(Rights::SAME_RIGHTS)
        .context("Failed to duplicate thread handle")?;
    set_role_for_target(RoleTarget::Thread(thread), role_name)
}

pub fn set_role_for_this_thread(role_name: &str) -> Result<(), Error> {
    fuchsia_runtime::with_thread_self(|thread| set_role_for_thread(thread, role_name))
}

pub fn set_role_for_vmar(vmar: &Vmar, role_name: &str) -> Result<(), Error> {
    let vmar =
        vmar.duplicate_handle(Rights::SAME_RIGHTS).context("Failed to duplicate vmar handle")?;
    set_role_for_target(RoleTarget::Vmar(vmar), role_name)
}

pub fn set_role_for_root_vmar(role_name: &str) -> Result<(), Error> {
    set_role_for_vmar(&fuchsia_runtime::vmar_root_self(), role_name)
}
