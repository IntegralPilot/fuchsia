// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides a transparent netstack proxy.
//!
//! The netstack proxy reads the network stack version it wants to use from
//! fuchsia.net.stackmigrationdeprecated.Control and spawns the appropriate
//! netstack binary from its own package.
//!
//! The directory request handle is passed directly to the spawned netstack.
//!
//! The incoming namespace for the spawned netstack is carefully constructed to
//! extract out the capabilities that are routed to netstack-proxy that are not
//! used by netstack itself.

use fidl::endpoints::{DiscoverableProtocolMarker, Proxy as _, RequestStream as _};
use fidl_fuchsia_net_stackmigrationdeprecated as fnet_migration;
use fidl_fuchsia_process_lifecycle as fprocess_lifecycle;
use fuchsia_async as fasync;
use futures::{FutureExt as _, StreamExt as _};
use std::pin::pin;
use vfs::directory::helper::DirectlyMutable;

#[fuchsia::main(logging = false)]
pub async fn main() -> std::process::ExitCode {
    // Start by getting the Netstack version we should use.
    let current_boot_version = {
        let migration =
            fuchsia_component::client::connect_to_protocol::<fnet_migration::StateMarker>()
                .expect("connect to protocol");
        let fnet_migration::InEffectVersion { current_boot, .. } =
            migration.get_netstack_version().await.expect("failed to read netstack version");
        current_boot
    };

    println!("netstack migration proxy using version {current_boot_version:?}");
    let bin_path = match current_boot_version {
        fnet_migration::NetstackVersion::Netstack2 => c"/pkg/bin/netstack",
        fnet_migration::NetstackVersion::Netstack3 => c"/pkg/bin/netstack3",
    };

    let ns = fdio::Namespace::installed().expect("failed to get namespace");
    let mut entries = ns
        .export()
        .expect("failed to export namespace entries")
        .into_iter()
        .filter_map(|fdio::NamespaceEntry { handle, path }| match path.as_str() {
            "/" => {
                panic!("unexpected non flat namespace, bad capabilities will bleed into netstack")
            }
            "/svc" => None,
            x => {
                Some((Some(handle), std::ffi::CString::new(x).expect("failed to create C string")))
            }
        })
        .collect::<Vec<_>>();

    let handle =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::DirectoryRequest.into())
            .expect("missing startup handle");

    let mut actions = vec![fdio::SpawnAction::add_handle(
        fuchsia_runtime::HandleInfo::new(fuchsia_runtime::HandleType::DirectoryRequest, 0),
        handle,
    )];

    actions.extend(entries.iter_mut().map(|(handle, path)| {
        // Handle is always Some here, we use an option so we can take it from
        // entries while entries keeps the CString backing.
        let handle = handle.take().unwrap();
        fdio::SpawnAction::add_namespace_entry(path.as_c_str(), handle)
    }));

    const LIFECYCLE_HANDLE_INFO: fuchsia_runtime::HandleInfo =
        fuchsia_runtime::HandleInfo::new(fuchsia_runtime::HandleType::Lifecycle, 0);
    let process_lifecycle = fuchsia_runtime::take_startup_handle(LIFECYCLE_HANDLE_INFO)
        .expect("missing lifecycle handle");

    let inner_lifecycle_proxy = match current_boot_version {
        // Netstack2 doesn't support clean shutdown.
        fnet_migration::NetstackVersion::Netstack2 => None,
        fnet_migration::NetstackVersion::Netstack3 => {
            // Create a proxy lifecycle channel that we'll use to tell netstack3
            // to stop.
            let (proxy, server) =
                fidl::endpoints::create_proxy::<fprocess_lifecycle::LifecycleMarker>();
            actions.push(fdio::SpawnAction::add_handle(
                LIFECYCLE_HANDLE_INFO,
                server.into_channel().into(),
            ));
            Some(proxy)
        }
    };

    let svc = vfs::directory::immutable::simple::simple();
    for s in std::fs::read_dir("/svc").expect("failed to get /svc entries") {
        let entry = s.expect("failed to get directory entry");
        let name = entry.file_name();
        let name = name.to_str().expect("failed to get file name");

        // Don't allow Netstack to see the services that we use exclusively to
        // enable proxying.
        let block_services = [
            fidl_fuchsia_process::LauncherMarker::PROTOCOL_NAME,
            fnet_migration::StateMarker::PROTOCOL_NAME,
        ];
        if block_services.into_iter().any(|s| s == name) {
            continue;
        }
        svc.add_entry(
            name,
            vfs::service::endpoint(move |_, channel| {
                fuchsia_component::client::connect_channel_to_protocol_at_path(
                    channel.into(),
                    entry.path().to_str().expect("failed to get entry path"),
                )
                .unwrap_or_else(|e| eprintln!("error connecting to protocol {:?}", e));
            }),
        )
        .unwrap_or_else(|e| panic!("failed to add entry {name}: {e:?}"));
    }

    let svc_dir = vfs::directory::serve_read_only(svc, vfs::execution_scope::ExecutionScope::new());
    let handle = svc_dir.into_client_end().unwrap().into();
    actions.push(fdio::SpawnAction::add_namespace_entry(c"/svc", handle));

    // Pass down the configuration VMO if we have it.
    let config_vmo_handle_info = fuchsia_runtime::HandleType::ComponentConfigVmo.into();
    if let Some(config_vmo) = fuchsia_runtime::take_startup_handle(config_vmo_handle_info) {
        actions.push(fdio::SpawnAction::add_handle(config_vmo_handle_info, config_vmo))
    }

    let log_sink_handle_info = fuchsia_runtime::HandleType::LogSink.into();
    if let Some(log_sink) = fuchsia_runtime::take_startup_handle(log_sink_handle_info) {
        actions.push(fdio::SpawnAction::add_handle(log_sink_handle_info, log_sink))
    }

    let netstack_process = fdio::spawn_etc(
        &fuchsia_runtime::job_default(),
        fdio::SpawnOptions::CLONE_ALL - fdio::SpawnOptions::CLONE_NAMESPACE,
        bin_path,
        &[bin_path],
        None,
        &mut actions[..],
    )
    .expect("failed to spawn netstack");

    let mut process_lifecycle = fprocess_lifecycle::LifecycleRequestStream::from_channel(
        fasync::Channel::from_channel(process_lifecycle.into()).into(),
    )
    .filter_map(|r| {
        futures::future::ready(match r {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("process lifecycle FIDL error {e:?}");
                None
            }
        })
    });

    let mut wait_signals = pin!(
        fasync::OnSignals::new(&netstack_process, zx::Signals::PROCESS_TERMINATED)
            .map(|s| s.expect("failed to observe process termination signals"))
    );
    let request = futures::select! {
        signals = wait_signals => {
            println!("netstack exited unexpectedly with {signals:?}");
            return std::process::ExitCode::FAILURE;
        },
        // If the stream terminates just wait for netstack to go away.
        r = process_lifecycle.select_next_some() => r,
    };

    let fprocess_lifecycle::LifecycleRequest::Stop { control_handle } = request;
    // Must drop the control_handle to unwrap the
    // lifecycle channel.
    std::mem::drop(control_handle);
    let (process_lifecycle, _terminated): (_, bool) = process_lifecycle.into_inner().into_inner();
    let process_lifecycle = std::sync::Arc::try_unwrap(process_lifecycle)
        .expect("failed to retrieve lifecycle channel");
    let process_lifecycle: zx::Channel = process_lifecycle.into_channel().into_zx_channel();
    if let Some(inner) = inner_lifecycle_proxy {
        inner
            .stop()
            .unwrap_or_else(|e| eprintln!("failed to request stop for inner netstack: {e:?}"));
        // Notify that we're done only on process exit.
        std::mem::forget(process_lifecycle);
    } else {
        // We're not proxying any channels, let component framework take us down
        // anytime.
        std::mem::drop(process_lifecycle);
    }

    let signals = wait_signals.await;
    assert!(signals.contains(zx::Signals::PROCESS_TERMINATED));
    // Process is terminated, mimic its exit code.
    let zx::ProcessInfo { return_code, .. } =
        netstack_process.info().expect("reading netstack process info");
    println!("netstack process exited with return code {return_code}");
    std::process::exit(return_code.try_into().unwrap_or(i32::MIN))
}
