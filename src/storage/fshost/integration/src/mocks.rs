// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::disk_builder::{DataSpec, DiskBuilder, VolumesSpec};
use anyhow::Error;
use fake_keymint::FakeKeymint;

use ffeedback::FileReportResults;
use fidl::prelude::*;
use fs_management::filesystem::DirBasedBlockConnector;
use fuchsia_component::client::connect::connect_to_named_protocol_at_dir_root;
use fuchsia_component_test::LocalComponentHandles;
use futures::channel::mpsc::{self};
use futures::future::BoxFuture;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _};
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

use fidl_fuchsia_boot as fboot;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_token as ftoken;
use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_fshost_fxfsprovisioner as ffxfsprovisioner;
use fidl_fuchsia_hardware_block_volume as fvolume;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_security_keymint as fkeymint;
use fidl_fuchsia_storage_partitions as fpartitions;
use test_vmo_backed_block_server::VmoBackedServer;
use vfs::directory::helper::DirectlyMutable;

pub fn new_mocks(
    vmo: Option<zx::Vmo>,
    crash_reports_sink: mpsc::Sender<ffeedback::CrashReport>,
    force_fxfs_provisioner_failure: bool,
    keymint: Arc<FakeKeymint>,
    simulated_gpt: Option<Arc<VmoBackedServer>>,
) -> impl Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), Error>> + Sync + Send + 'static
{
    let vmo = vmo.map(Arc::new);
    let mock = move |handles: LocalComponentHandles| {
        let vmo_clone = vmo.clone();
        let keymint_clone = keymint.clone();
        let simulated_gpt_clone = simulated_gpt.clone();
        run_mocks(
            handles,
            vmo_clone,
            crash_reports_sink.clone(),
            force_fxfs_provisioner_failure,
            keymint_clone,
            simulated_gpt_clone,
        )
        .boxed()
    };

    mock
}

async fn run_mocks(
    handles: LocalComponentHandles,
    vmo: Option<Arc<zx::Vmo>>,
    crash_reports_sink: mpsc::Sender<ffeedback::CrashReport>,
    force_fxfs_provisioner_failure: bool,
    keymint_instance: Arc<FakeKeymint>,
    simulated_gpt: Option<Arc<VmoBackedServer>>,
) -> Result<(), Error> {
    let keymint_for_sealing = keymint_instance.clone();
    let keymint_for_admin = keymint_instance.clone();

    let svc_dir = vfs::pseudo_directory! {
        fkeymint::SealingKeysMarker::PROTOCOL_NAME => vfs::service::host(
            move |stream| {
                run_keymint(stream, keymint_for_sealing.clone()).boxed()
            }
        ),
        fkeymint::AdminMarker::PROTOCOL_NAME => vfs::service::host(
            move |stream| {
                run_keymint_admin(stream, keymint_for_admin.clone()).boxed()
            }
        ),
        fboot::ItemsMarker::PROTOCOL_NAME => vfs::service::host(move |stream| {
            let vmo_clone = vmo.clone();
            run_boot_items(stream, vmo_clone)
        }),
        ffeedback::CrashReporterMarker::PROTOCOL_NAME => vfs::service::host(move |stream| {
            run_crash_reporter(stream, crash_reports_sink.clone())
        }),
        ffxfsprovisioner::FxfsProvisionerMarker::PROTOCOL_NAME => vfs::service::host(
            move |stream| {
                run_fxfs_provisioner(
                    stream, force_fxfs_provisioner_failure, keymint_instance.clone())
            }
        ),
        ftoken::NodeBusTopologyMarker::PROTOCOL_NAME => vfs::service::host(move |stream| {
            run_node_bus_topology(stream)
        }),
    };

    if let Some(server) = simulated_gpt {
        let server_clone1 = server.clone();
        let server_clone2 = server.clone();
        svc_dir
            .add_entry(
                fvolume::ServiceMarker::SERVICE_NAME,
                vfs::pseudo_directory! {
                    "system_gpt" => vfs::pseudo_directory! {
                        "volume" => vfs::service::host(move |stream| {
                            let server = server_clone1.clone();
                            async move {
                                let _ = server.serve(stream).await;
                            }
                        }),
                        "inline_encryption" => vfs::service::host(move |stream| {
                            let server = server_clone2.clone();
                            async move {
                                server.serve_insecure_inline_encryption(stream, [0; 16]).await;
                            }
                        }),
                    },
                },
            )
            .unwrap();
    }

    let export = vfs::pseudo_directory! {
        "svc" => svc_dir,
    };

    let scope = ExecutionScope::new();
    vfs::directory::serve_on(export, fio::PERM_READABLE, scope.clone(), handles.outgoing_dir);
    scope.wait().await;

    Ok(())
}

/// fshost uses exactly one boot item - it checks to see if there is an item of type
/// ZBI_TYPE_STORAGE_RAMDISK. If it's there, it's a vmo that represents a ramdisk version of the
/// fvm, and fshost creates a ramdisk from the vmo so it can go through the normal device matching.
async fn run_boot_items(mut stream: fboot::ItemsRequestStream, vmo: Option<Arc<zx::Vmo>>) {
    while let Some(request) = stream.next().await {
        match request.unwrap() {
            fboot::ItemsRequest::Get { type_, extra, responder } => {
                assert_eq!(type_, zbi::Type::StorageRamdisk as u32);
                assert_eq!(extra, 0);
                let response_vmo = vmo.as_ref().map(|vmo| {
                    vmo.create_child(zx::VmoChildOptions::SLICE, 0, vmo.get_size().unwrap())
                        .unwrap()
                });
                responder.send(response_vmo, 0).unwrap();
            }
            fboot::ItemsRequest::Get2 { type_, extra, responder } => {
                assert_eq!(type_, zbi::Type::StorageRamdisk as u32);
                assert_eq!((*extra.unwrap()).n, 0);
                responder.send(Ok(Vec::new())).unwrap();
            }
            fboot::ItemsRequest::GetBootloaderFile { .. } => {
                panic!(
                    "unexpectedly called GetBootloaderFile on {}",
                    fboot::ItemsMarker::PROTOCOL_NAME
                );
            }
        }
    }
}

async fn run_crash_reporter(
    mut stream: ffeedback::CrashReporterRequestStream,
    mut crash_reports_sink: mpsc::Sender<ffeedback::CrashReport>,
) {
    while let Some(request) = stream.next().await {
        match request.unwrap() {
            ffeedback::CrashReporterRequest::FileReport { report, responder } => {
                crash_reports_sink.send(report).await.unwrap();
                responder.send(Ok(&FileReportResults::default())).unwrap();
            }
        }
    }
}

async fn run_fxfs_provisioner(
    mut stream: ffxfsprovisioner::FxfsProvisionerRequestStream,
    force_failure: bool,
    keymint: Arc<FakeKeymint>,
) {
    while let Some(request) = stream.next().await {
        match request.unwrap() {
            ffxfsprovisioner::FxfsProvisionerRequest::Provision {
                partition_service,
                responder,
            } => {
                if force_failure {
                    responder.send(Err(zx::Status::INTERNAL.into_raw())).unwrap();
                    return;
                }
                let partition_service = partition_service.into_proxy();

                let overlay = connect_to_named_protocol_at_dir_root::<
                    fpartitions::OverlayPartitionProxy,
                >(&partition_service, "overlay")
                .expect("failed to connect to OverlayPartition protocol");
                let partitions_info = overlay
                    .get_partitions()
                    .await
                    .expect("get_partitions FIDL call failed")
                    .expect("get_partitions failed");
                assert_eq!(partitions_info.len(), 2);
                assert!(partitions_info.iter().any(|info| info.name.as_deref() == Some("super")));
                assert!(
                    partitions_info.iter().any(|info| info.name.as_deref() == Some("userdata"))
                );

                let connector =
                    Box::new(DirBasedBlockConnector::new(partition_service, "/volume".to_string()));

                let mut disk_builder = DiskBuilder::new();
                disk_builder.with_keymint_instance(keymint.clone());
                disk_builder
                    .format_volumes(VolumesSpec { fxfs_blob: true, create_data_partition: true })
                    .format_data(DataSpec { format: Some("fxfs"), ..Default::default() });
                disk_builder.build_fxfs_as_volume_manager(connector).await;

                responder.send(Ok(())).unwrap();
            }
            _ => {
                unreachable!()
            }
        }
    }
}

async fn run_keymint(stream: fkeymint::SealingKeysRequestStream, keymint: Arc<FakeKeymint>) {
    keymint.run_sealing_keys_service(stream).await.unwrap();
}

async fn run_keymint_admin(stream: fkeymint::AdminRequestStream, keymint: Arc<FakeKeymint>) {
    keymint.run_admin_service(stream).await.unwrap();
}

async fn run_node_bus_topology(mut stream: ftoken::NodeBusTopologyRequestStream) {
    while let Some(request) = stream.next().await {
        match request.unwrap() {
            ftoken::NodeBusTopologyRequest::Get { token: _, responder } => {
                let path = vec![fdf::BusInfo {
                    bus: Some(fdf::BusType::Platform),
                    address: Some(fdf::DeviceAddress::IntValue(1)),
                    address_stability: Some(fdf::DeviceAddressStability::Stable),
                    ..Default::default()
                }];
                responder.send(Ok(&path)).unwrap();
            }
            _ => {
                unreachable!()
            }
        }
    }
}
