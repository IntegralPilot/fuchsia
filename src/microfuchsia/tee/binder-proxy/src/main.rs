// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod binder_proxy;
mod bound_virtio_socket;
mod convert;
mod microfuchsia_control;
mod ta_rpc_session;
mod trusted_app;

use anyhow::{self, Context, Error};
use android_system_microfuchsia_vm_service::aidl::android::system::microfuchsia::vm_service::IMicrofuchsia::GUEST_PORT;
use fuchsia_component::client::Service;

unsafe extern "C" {
    fn register_dev_urandom_compat() -> zx::sys::zx_status_t;
}

fn get_pmem_buffer() -> Result<zx::Vmo, Error> {
    fuchsia_async::LocalExecutor::default().run_singlethreaded((async || {
        log::info!("waiting for virtiopmem");
        let virtio_pmem = Service::open(fidl_fuchsia_hardware_virtio_pmem::ServiceMarker)
            .context("Failed to open service")?
            .watch_for_any()
            .await
            .context("Failed to find instance")?
            .connect_to_device()
            .context("Failed to connect to device protocol")?;

        let fidl_result = virtio_pmem.get().await?;
        fidl_result.map_err(|e| anyhow::anyhow!("fidl error: {e}"))
    })())
}

#[fuchsia::main]
fn main() -> Result<(), Error> {
    log::info!("binder-proxy main");
    // Call register_dev_urandom_compat
    if let Err(status) = zx::Status::ok(unsafe { register_dev_urandom_compat() }) {
        anyhow::bail!("Could not register /dev/urandom compatibility device: {status}");
    }

    let config = binder_proxy_config::Config::take_from_startup_handle();

    let shared_mem_vmo = if config.use_virtio_pmem { Some(get_pmem_buffer()?) } else { None };

    let binder_proxy = binder_proxy::BinderProxy::new(&config, GUEST_PORT as u32, shared_mem_vmo)?;

    // We need to keep the RPCSessions alive for as long as the proxy itself is running.
    #[allow(clippy::collection_is_never_read)]
    let mut ta_rpc_sessions = vec![];
    // For each TA exposed by the manager, set up an rpc server bound to the corresponding
    // port above GUEST_PORT and listen for incoming connections.
    // Then .start() each TA specific RPC server.
    let uuids = std::fs::read_dir("/ta")
        .context("Reading /ta directory")?
        .map(|entry| entry.unwrap().file_name())
        .map(|s| s.into_string().unwrap())
        .collect::<Vec<_>>();
    for i in 0..uuids.len() {
        let port = GUEST_PORT as u32 + 1 + i as u32;
        let ta_rpc_session = ta_rpc_session::RPCSession::new(&config, port, &uuids[i])
            .context("Starting RPC session for uuid {uuid} on port {port}")?;
        ta_rpc_session.start();
        ta_rpc_sessions.push(ta_rpc_session);
    }
    binder_proxy.run()?;
    Ok(())
}
