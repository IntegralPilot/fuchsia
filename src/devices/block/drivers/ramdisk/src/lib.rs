// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod ramdisk;

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fidl::endpoints::{RequestStream, ServiceMarker};
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_hardware_ramdisk as framdisk;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::warn;
use ramdisk::Ramdisk;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::simple::Simple;
use vfs::execution_scope::ExecutionScope;
use vfs::service::endpoint;
use zx::{self, Status};

struct RamdiskController {
    scope: ExecutionScope,
}

struct RamdiskControllerInner {
    node: Node,
    counter: AtomicI32,
    volume_svc_dir: Arc<Simple>,
    scope: ExecutionScope,
    node_token: Option<zx::Event>,
}

driver_register!(RamdiskController);

impl RamdiskControllerInner {
    async fn handle_controller_request(
        self: &Arc<Self>,
        request: framdisk::ControllerRequest,
    ) -> Result<(), fidl::Error> {
        match request {
            framdisk::ControllerRequest::Create { payload, responder } => {
                let result = self.create(payload).await;
                responder.send(result.map_err(|s| s.into_raw()))?;
            }
            framdisk::ControllerRequest::_UnknownMethod { .. } => {
                warn!("Unknown controller request: {request:?}");
            }
        }
        Ok(())
    }

    async fn create(
        self: &Arc<Self>,
        options: framdisk::Options,
    ) -> Result<(fidl::endpoints::ClientEnd<fio::DirectoryMarker>, zx::EventPair), Status> {
        let block_size = options.block_size.unwrap_or_else(|| zx::system_get_page_size() as u32);
        if block_size == 0 {
            return Err(Status::INVALID_ARGS);
        }

        let (vmo, block_count) = if let Some(vmo) = options.vmo {
            let block_count = if let Some(count) = options.block_count {
                count.checked_mul(block_size as u64).ok_or(Status::INVALID_ARGS)?;
                count
            } else {
                let vmo_size = vmo.get_size().map_err(|_| Status::INTERNAL)?;
                vmo_size / block_size as u64
            };
            (vmo, block_count)
        } else {
            let block_count = options.block_count.ok_or(Status::INVALID_ARGS)?;
            let size = block_count.checked_mul(block_size as u64).ok_or(Status::INVALID_ARGS)?;
            let vmo = zx::Vmo::create(size).map_err(|_| Status::INTERNAL)?;
            (vmo, block_count)
        };

        let partition_info = block_server::PartitionInfo {
            device_flags: options
                .device_flags
                .unwrap_or_else(fidl_fuchsia_storage_block::DeviceFlag::empty),
            max_transfer_blocks: options.max_transfer_blocks.and_then(std::num::NonZeroU32::new),
            block_count,
            type_guid: options.type_guid.map(|g| g.value).unwrap_or([0; 16]),
            ..Default::default()
        };

        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        let node_name = format!("ramdisk-{}", id);

        let (node_controller, node_controller_server) =
            fidl::endpoints::create_proxy::<fdf::NodeControllerMarker>();

        let (ramdisk_client, ramdisk_server) =
            fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
        let scope = ExecutionScope::new();
        let node_token = self
            .node_token
            .as_ref()
            .map(|t| t.duplicate_handle(zx::Rights::SAME_RIGHTS))
            .transpose()
            .map_err(|_| Status::INTERNAL)?;
        let instance = Ramdisk::new(scope.clone(), vmo, partition_info, block_size, node_token)?;
        instance.serve(&scope, ramdisk_server);

        let publish = options.publish.unwrap_or(false);

        if publish {
            let instance_dir = Simple::new();

            instance_dir.add_entry("volume", endpoint(instance.block_request_handler())).map_err(
                |error| {
                    warn!(error:?; "Failed to add volume entry");
                    Status::INTERNAL
                },
            )?;

            instance_dir.add_entry("token", endpoint(instance.token_request_handler())).map_err(
                |error| {
                    warn!(error:?; "Failed to add token entry");
                    Status::INTERNAL
                },
            )?;

            self.volume_svc_dir.add_entry(id.to_string(), instance_dir).map_err(|error| {
                warn!(id, error:?; "Failed to add instance entry");
                Status::INTERNAL
            })?;
        }

        let (endpoint0, endpoint1) = zx::EventPair::create();

        match self
            .node
            .proxy()
            .add_child(
                fdf::NodeAddArgs { name: Some(node_name), ..Default::default() },
                node_controller_server,
                None,
            )
            .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                warn!(error:?; "add_child failed");
                return Err(Status::INTERNAL);
            }
            Err(error) => {
                warn!(error:?; "add_child FIDL error");
                return Err(Status::INTERNAL);
            }
        }
        let guard = scopeguard::guard(node_controller, move |nc| {
            let _ = nc.remove();
            scope.shutdown();
        });

        // Watch the eventpair so that when it is dropped, the ramdisk instance is destroyed.
        let inner_clone = self.clone();
        self.scope.spawn(async move {
            let _guard = guard;
            let _instance = instance;
            let _ = fasync::OnSignals::new(&endpoint1, zx::Signals::EVENTPAIR_PEER_CLOSED).await;
            let _ = inner_clone.volume_svc_dir.remove_entry(id.to_string(), false);
            log::info!("Destroyed ramdisk {id}");
        });

        log::info!("Created ramdisk {id} {}", if publish { "" } else { " (unpublished)" });

        Ok((ramdisk_client, endpoint0))
    }
}

impl Driver for RamdiskController {
    const NAME: &str = "ramdisk";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;
        let node_token = context.start_args.node_token.take();
        let volume_svc_dir = Simple::new();
        let scope = ExecutionScope::new();
        let inner = Arc::new(RamdiskControllerInner {
            node,
            counter: AtomicI32::new(0),
            volume_svc_dir: volume_svc_dir.clone(),
            scope: scope.clone(),
            node_token,
        });

        let mut fs = ServiceFs::new();
        let inner_clone = inner.clone();
        let scope_clone = scope.clone();
        let mut svc_dir = fs.dir("svc");
        svc_dir.dir(framdisk::ServiceMarker::SERVICE_NAME).dir("default").add_service_at(
            "controller",
            move |channel: zx::Channel| {
                let inner_clone = inner_clone.clone();
                let requests = framdisk::ControllerRequestStream::from_channel(
                    fasync::Channel::from_channel(channel),
                );
                scope_clone.spawn(async move {
                    let _ = requests
                        .try_for_each(|request| inner_clone.handle_controller_request(request))
                        .await;
                });
                Some(())
            },
        );
        svc_dir.add_entry_at(
            fidl_fuchsia_hardware_block_volume::ServiceMarker::SERVICE_NAME,
            volume_svc_dir,
        );

        context.serve_outgoing(&mut fs)?;
        scope.spawn(fs.collect());
        Ok(RamdiskController { scope })
    }

    async fn stop(&self) {
        self.scope.shutdown();
    }
}
