// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use bootloader_message::{BootloaderMessage, BootloaderMessageRaw};
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_fshost::{RecoveryMarker, RecoveryRequest, RecoveryRequestStream};
use fidl_fuchsia_hardware_adb as fadb;
use fidl_fuchsia_hardware_display as fdisplay;
use fidl_fuchsia_hardware_power_statecontrol::{
    AdminMarker, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fidl_fuchsia_input_report as finput;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_recovery::{FactoryResetMarker, FactoryResetRequest, FactoryResetRequestStream};
use fidl_fuchsia_recovery_android as frecovery_android;
use fidl_fuchsia_storage_block as fstorage_block;
use fidl_fuchsia_sysmem2 as fsysmem2;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::channel::mpsc;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _};
use mock_reboot::MockRebootService;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use vfs::execution_scope::ExecutionScope;
use vmo_backed_block_server::VmoBackedServer;
use zerocopy::IntoBytes as _;

struct TestEnvBuilder {
    recovery_args: String,
}

impl TestEnvBuilder {
    fn new() -> Self {
        Self { recovery_args: "".into() }
    }

    fn recovery_args(mut self, args: impl Into<String>) -> Self {
        self.recovery_args = args.into();
        self
    }

    async fn build(self) -> TestEnv {
        let builder = RealmBuilder::new().await.unwrap();
        let system_recovery = builder
            .add_child(
                "system_recovery",
                "#meta/system_recovery_android.cm",
                ChildOptions::new().eager(),
            )
            .await
            .unwrap();

        let (reboot_sender, reboot_receiver) = mpsc::channel(1);
        let reboot_service = Arc::new(MockRebootService::new(Box::new(move |options| {
            reboot_sender.clone().try_send(options).unwrap();
            Ok(())
        })));

        let (factory_reset_sender, factory_reset_receiver) = mpsc::channel(1);

        let msg: BootloaderMessageRaw =
            BootloaderMessage::with_args(&self.recovery_args).try_into().unwrap();
        let vmo_server = Arc::new(VmoBackedServer::new(8, 512, msg.as_bytes()).unwrap());

        let mount_called = Arc::new(AtomicU32::new(0));
        let mock_fshost_mount_called = Arc::clone(&mount_called);

        let service_reflector_root = vfs::pseudo_directory! {
            "block" => vfs::pseudo_directory! {
                "misc" => vfs::pseudo_directory! {
                    fstorage_block::BlockMarker::PROTOCOL_NAME => vfs::service::host(move |stream| {
                        let vmo_server = Arc::clone(&vmo_server);
                        async move {
                            let () = vmo_server.serve(stream).await.unwrap();
                        }
                    }),
                }
            },
            "svc" => vfs::pseudo_directory! {
                FactoryResetMarker::PROTOCOL_NAME => vfs::service::host(move |mut stream: FactoryResetRequestStream| {
                    let mut factory_reset_sender = factory_reset_sender.clone();
                    async move {
                        while let Some(Ok(request)) = stream.next().await {
                            factory_reset_sender.send(request).await.unwrap();
                        }
                    }
                }),
                AdminMarker::PROTOCOL_NAME => vfs::service::host(move |stream| {
                    let reboot_service = Arc::clone(&reboot_service);
                    async move {
                        let () = reboot_service.run_reboot_service(stream).await.unwrap();
                    }
                }),
                fadb::StateControllerMarker::PROTOCOL_NAME => vfs::service::host(move |mut stream: fadb::StateControllerRequestStream| {
                    async move {
                        while let Some(Ok(request)) = stream.next().await {
                            match request {
                                fadb::StateControllerRequest::SetSystemType { system_type: _, responder } => {
                                    responder.send(Ok(())).unwrap();
                                }
                                _ => {}
                            }
                        }
                    }
                }),
                RecoveryMarker::PROTOCOL_NAME => vfs::service::host(move |mut stream: RecoveryRequestStream| {
                    let mount_called = Arc::clone(&mock_fshost_mount_called);
                    async move {
                        while let Some(Ok(RecoveryRequest::MountSystemBlobVolume {
                            blob_exposed_dir: _,
                            responder,
                        })) = stream.next().await
                        {
                            mount_called.fetch_add(1, Ordering::SeqCst);
                            responder.send(Ok(())).unwrap();
                        }
                    }
                }),
            },
        };

        let service_reflector = builder
            .add_local_child(
                "service_reflector",
                move |handles| {
                    let scope = ExecutionScope::new();
                    vfs::directory::serve_on(
                        service_reflector_root.clone(),
                        fio::PERM_READABLE | fio::PERM_WRITABLE,
                        scope.clone(),
                        handles.outgoing_dir,
                    );
                    async move { Ok(scope.wait().await) }.boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("block").path("/block").rights(fio::R_STAR_DIR),
                    )
                    .capability(Capability::protocol::<FactoryResetMarker>())
                    .capability(Capability::protocol::<AdminMarker>())
                    .capability(Capability::protocol::<RecoveryMarker>())
                    .capability(Capability::protocol::<fadb::StateControllerMarker>())
                    .from(&service_reflector)
                    .to(&system_recovery),
            )
            .await
            .unwrap();

        let fake_display = builder
            .add_child("fake-display", "#meta/fake-display-stack-host.cm", ChildOptions::new())
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::service::<fdisplay::ServiceMarker>())
                    .from(&fake_display)
                    .to(&system_recovery),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fsysmem2::AllocatorMarker>())
                    .from(Ref::parent())
                    .to(&fake_display),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::service::<finput::ServiceMarker>())
                    .capability(Capability::protocol::<fsysmem2::AllocatorMarker>())
                    .from(Ref::parent())
                    .to(&system_recovery),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.recovery.DisplayRotation"))
                    .from(Ref::void())
                    .to(&system_recovery),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<frecovery_android::UpdaterMarker>())
                    .from(&system_recovery)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        let realm = builder.build().await.unwrap();
        TestEnv { realm, mount_called, reboot_receiver, factory_reset_receiver }
    }
}

struct TestEnv {
    realm: RealmInstance,
    mount_called: Arc<AtomicU32>,
    reboot_receiver: mpsc::Receiver<ShutdownOptions>,
    factory_reset_receiver: mpsc::Receiver<FactoryResetRequest>,
}

impl TestEnv {
    fn updater_proxy(&self) -> frecovery_android::UpdaterProxy {
        self.realm.root.connect_to_protocol_at_exposed_dir().expect("connect to updater")
    }

    fn mount_called(&self) -> u32 {
        self.mount_called.load(Ordering::SeqCst)
    }

    async fn wait_for_reboot(&mut self) -> ShutdownOptions {
        self.reboot_receiver.next().await.expect("wait for reboot")
    }

    async fn wait_for_factory_reset_request(&mut self) -> FactoryResetRequest {
        self.factory_reset_receiver.next().await.expect("wait for factory reset request")
    }
}

#[fuchsia::test]
async fn test_wipe_data() {
    let mut env = TestEnvBuilder::new().recovery_args("--wipe_data\n").build().await;
    match env.wait_for_factory_reset_request().await {
        FactoryResetRequest::Reset { responder } => {
            responder.send(zx::sys::ZX_OK).unwrap();
        }
    }
}

#[fuchsia::test]
async fn test_sideload_auto_reboot() {
    let mut env = TestEnvBuilder::new().recovery_args("--sideload_auto_reboot\n").build().await;

    // This returns after update has finished, but the update is expected to fail, because the test
    // doesn't provide all the dependencies needed for a successful update.
    let () = env.updater_proxy().update("https://fuchsia.com/ota_manifest").await.unwrap();
    assert_eq!(env.mount_called(), 1);

    let reboot_options = env.wait_for_reboot().await;
    assert_eq!(reboot_options.action, Some(ShutdownAction::Reboot));
    assert_eq!(reboot_options.reasons, Some(vec![ShutdownReason::DeveloperRequest]));
}
