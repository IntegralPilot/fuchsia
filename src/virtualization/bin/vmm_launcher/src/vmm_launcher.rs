// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component::{CreateChildArgs, RealmProxy};
use fidl_fuchsia_virtualization::GuestLifecycleMarker;
use fuchsia_component::client;
use futures::stream::Stream;
use futures::StreamExt;
use {fidl_fuchsia_component_decl as cdecl, fidl_fuchsia_io as fio, zx_status};

fn send_epitaph<T>(server_end: ServerEnd<T>, epitaph: zx_status::Status) {
    if let Err(e) = server_end.close_with_epitaph(epitaph) {
        log::error!("Unable to write epitaph{}: {}", epitaph, e);
    }
}

pub struct VmmLauncher {
    realm: RealmProxy,
    next_instance_id: usize,
    vmm_component_url: String,
}

impl VmmLauncher {
    pub fn new(vmm_component_url: String, realm: RealmProxy) -> Self {
        Self { realm, next_instance_id: 1, vmm_component_url }
    }

    pub async fn run<St: Stream<Item = ServerEnd<GuestLifecycleMarker>> + Unpin>(
        &mut self,
        mut stream: St,
    ) {
        while let Some(server_end) = stream.next().await {
            self.spawn_vmm(server_end).await;
        }
    }

    fn increment_instance_id(&mut self) -> usize {
        let id = self.next_instance_id;
        self.next_instance_id += 1;
        id
    }

    async fn spawn_vmm(&mut self, lifecycle: ServerEnd<GuestLifecycleMarker>) {
        let child_name = format!("vmm-{}", self.increment_instance_id());
        let collection_name = "virtual_machine_managers";
        let collection_ref = cdecl::CollectionRef { name: collection_name.to_string() };
        let decl = cdecl::Child {
            name: Some(child_name.clone()),
            url: Some(self.vmm_component_url.clone()),
            startup: Some(cdecl::StartupMode::Lazy),
            on_terminate: Some(cdecl::OnTerminate::None),
            ..Default::default()
        };
        let args = CreateChildArgs::default();
        let realm_result = self.realm.create_child(&collection_ref, &decl, args).await;
        match realm_result {
            Err(fidl_error) => {
                log::error!("FIDL error creating vmm child: {}", fidl_error);
                send_epitaph(lifecycle, zx_status::Status::INTERNAL);
                return;
            }
            Ok(Err(realm_error)) => {
                log::error!("Realm error creating vmm child: {:?}", realm_error);
                send_epitaph(lifecycle, zx_status::Status::NO_RESOURCES);
                return;
            }
            Ok(Ok(())) => {}
        }

        // Connect to exposed service directory for the vmm.
        let (svc_dir_proxy, svc_dir) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let child_ref =
            cdecl::ChildRef { name: child_name, collection: Some(collection_name.to_string()) };
        let realm_result = self.realm.open_exposed_dir(&child_ref, svc_dir).await;
        match realm_result {
            Err(fidl_error) => {
                log::error!("FIDL error opening vmm child exposed dir: {}", fidl_error);
                send_epitaph(lifecycle, zx_status::Status::INTERNAL);
                return;
            }
            Ok(Err(realm_error)) => {
                log::error!("Realm error opening vmm child exposed dir: {:?}", realm_error);
                send_epitaph(lifecycle, zx_status::Status::INTERNAL);
                return;
            }
            Ok(Ok(())) => {}
        }

        let connect_result =
            client::new_protocol_connector_in_dir::<GuestLifecycleMarker>(&svc_dir_proxy)
                .connect_with(lifecycle.into_channel());
        if let Err(e) = connect_result {
            log::error!("Failed to connect to GuestLifecycle in vmm child: {}", e);
        }
    }
}
