// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use attribution_server::{AttributionServer, AttributionServerHandle};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_memory_attribution as fma;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use fuchsia_sync::Mutex;
use futures::StreamExt;
use log::warn;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

struct DriverInfo {
    component_token: zx::Event,
    process_koid: zx::Koid,
}

pub struct MemoryAttributor {
    drivers: Arc<Mutex<HashMap<u64, DriverInfo>>>,
    attribution_server: AttributionServerHandle,
}

impl driver_manager_node::MemoryAttributor for MemoryAttributor {
    fn add_driver(&self, component_token: zx::Event, id: u64, process_koid: zx::Koid) {
        let token_dup = component_token
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("Failed to duplicate component token");
        {
            let mut drivers = self.drivers.lock();
            drivers.insert(id, DriverInfo { component_token, process_koid });
        }

        self.attribution_server.new_publisher().on_update(vec![
            fma::AttributionUpdate::Add(fma::NewPrincipal {
                identifier: Some(id),
                description: Some(fma::Description::Component(token_dup)),
                principal_type: Some(fma::PrincipalType::Runnable),
                ..Default::default()
            }),
            fma::AttributionUpdate::Update(fma::UpdatedPrincipal {
                identifier: Some(id),
                resources: Some(fma::Resources::Data(fma::Data {
                    resources: vec![fma::Resource::KernelObject(process_koid.raw_koid())],
                })),
                ..Default::default()
            }),
        ]);
    }

    fn remove_driver(&self, id: u64) {
        if self.drivers.lock().remove(&id).is_some() {
            self.attribution_server
                .new_publisher()
                .on_update(vec![fma::AttributionUpdate::Remove(id)]);
        }
    }
}

impl MemoryAttributor {
    pub fn new() -> Self {
        let drivers = Arc::new(Mutex::new(HashMap::<u64, DriverInfo>::new()));
        let drivers_clone = drivers.clone();
        let attribution_server = AttributionServer::new(Box::new(move || {
            let drivers = drivers_clone.lock();
            drivers
                .iter()
                .flat_map(|(&id, info)| {
                    let token = info
                        .component_token
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("Failed to duplicate component token");
                    vec![
                        fma::AttributionUpdate::Add(fma::NewPrincipal {
                            identifier: Some(id),
                            description: Some(fma::Description::Component(token)),
                            principal_type: Some(fma::PrincipalType::Runnable),
                            ..Default::default()
                        }),
                        fma::AttributionUpdate::Update(fma::UpdatedPrincipal {
                            identifier: Some(id),
                            resources: Some(fma::Resources::Data(fma::Data {
                                resources: vec![fma::Resource::KernelObject(
                                    info.process_koid.raw_koid(),
                                )],
                            })),
                            ..Default::default()
                        }),
                    ]
                })
                .collect()
        }));
        Self { drivers, attribution_server }
    }

    pub fn publish(self: &Rc<Self>, fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>) {
        let this = self.clone();
        fs.dir("svc").add_fidl_service(move |stream: fma::ProviderRequestStream| {
            let this = this.clone();
            let observer = this.attribution_server.new_observer(stream.control_handle());
            fasync::Task::local(async move {
                if let Err(e) = this.serve(observer, stream).await {
                    warn!("Failed to serve MemoryAttributor: {}", e);
                }
            })
            .detach();
        });
    }

    async fn serve(
        &self,
        observer: attribution_server::Observer,
        mut stream: fma::ProviderRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.next().await {
            match request? {
                fma::ProviderRequest::Get { responder } => {
                    observer.next(responder);
                }
                fma::ProviderRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(
                        "fuchsia.memory.attribution/Provider received unknown method: {}",
                        ordinal
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_manager_node::MemoryAttributor as _;
    use fidl::endpoints::create_proxy_and_stream;
    use futures::FutureExt;

    #[fuchsia::test]
    async fn test_add_driver() {
        let attributor = Rc::new(MemoryAttributor::new());
        let (proxy, stream) = create_proxy_and_stream::<fma::ProviderMarker>();

        let attributor_clone = attributor.clone();
        let observer = attributor_clone.attribution_server.new_observer(stream.control_handle());
        fasync::Task::local(async move {
            attributor_clone.serve(observer, stream).await.unwrap();
        })
        .detach();

        // 1. Get request should hang if no updates/principals
        let mut get_fut = proxy.get().fuse();
        futures::select! {
            _ = get_fut => panic!("Get should have hung"),
            _ = fasync::Timer::new(zx::MonotonicDuration::from_millis(100)).fuse() => (),
        }

        // 2. Add a driver
        let token = zx::Event::create();
        let id = 123;
        let process_koid = zx::Koid::from_raw(456);
        attributor.add_driver(token, id, process_koid);

        // 3. Now Get should complete
        let result = get_fut.await.unwrap().unwrap();
        let attributions = result.attributions.unwrap();
        assert_eq!(attributions.len(), 2);

        match &attributions[0] {
            fma::AttributionUpdate::Add(new_principal) => {
                assert_eq!(new_principal.identifier.unwrap(), id);
            }
            _ => panic!("Expected Add update"),
        }

        match &attributions[1] {
            fma::AttributionUpdate::Update(updated_principal) => {
                assert_eq!(updated_principal.identifier.unwrap(), id);
                let resources = updated_principal.resources.as_ref().unwrap();
                match resources {
                    fma::Resources::Data(data) => {
                        assert_eq!(data.resources.len(), 1);
                        match &data.resources[0] {
                            fma::Resource::KernelObject(koid) => assert_eq!(*koid, 456),
                            _ => panic!("Expected KernelObject resource"),
                        }
                    }
                    _ => panic!("Expected Data resources"),
                }
            }
            _ => panic!("Expected Update update"),
        }
    }

    #[fuchsia::test]
    async fn test_remove_driver() {
        let attributor = Rc::new(MemoryAttributor::new());
        let (proxy, stream) = create_proxy_and_stream::<fma::ProviderMarker>();

        let attributor_clone = attributor.clone();
        let observer = attributor_clone.attribution_server.new_observer(stream.control_handle());
        fasync::Task::local(async move {
            attributor_clone.serve(observer, stream).await.unwrap();
        })
        .detach();

        // 1. Add a driver first
        let id = 123;
        attributor.add_driver(zx::Event::create(), id, zx::Koid::from_raw(456));

        // 2. Get initial state
        let result = proxy.get().await.unwrap().unwrap();
        assert_eq!(result.attributions.unwrap().len(), 2);

        // 3. Get request should hang if no further updates
        let mut get_fut = proxy.get().fuse();
        futures::select! {
            _ = get_fut => panic!("Get should have hung"),
            _ = fasync::Timer::new(zx::MonotonicDuration::from_millis(100)).fuse() => (),
        }

        // 4. Remove the driver
        attributor.remove_driver(id);

        // 5. Now Get should complete
        let result = get_fut.await.unwrap().unwrap();
        let attributions = result.attributions.unwrap();
        assert_eq!(attributions.len(), 1);

        match &attributions[0] {
            fma::AttributionUpdate::Remove(removed_id) => {
                assert_eq!(*removed_id, id);
            }
            _ => panic!("Expected Remove update"),
        }
    }

    #[fuchsia::test]
    async fn test_get_immediate() {
        let attributor = Rc::new(MemoryAttributor::new());
        let (proxy, stream) = create_proxy_and_stream::<fma::ProviderMarker>();

        let attributor_clone = attributor.clone();
        let observer = attributor_clone.attribution_server.new_observer(stream.control_handle());
        fasync::Task::local(async move {
            attributor_clone.serve(observer, stream).await.unwrap();
        })
        .detach();

        let id = 123;
        attributor.add_driver(zx::Event::create(), id, zx::Koid::from_raw(456));

        // Get should return immediately since there is a pending update (the initial state)
        let result = proxy.get().await.unwrap().unwrap();
        assert_eq!(result.attributions.unwrap().len(), 2);
    }

    #[fuchsia::test]
    async fn test_multiple_gets_fail() {
        let attributor = Rc::new(MemoryAttributor::new());
        let (proxy, stream) = create_proxy_and_stream::<fma::ProviderMarker>();

        let attributor_clone = attributor.clone();
        let observer = attributor_clone.attribution_server.new_observer(stream.control_handle());
        fasync::Task::local(async move {
            attributor_clone.serve(observer, stream).await.unwrap();
        })
        .detach();

        // First get hangs because state is empty
        let mut get_fut1 = proxy.get().fuse();
        futures::select! {
            _ = get_fut1 => panic!("Get should have hung"),
            _ = fasync::Timer::new(zx::MonotonicDuration::from_millis(100)).fuse() => (),
        }

        // Second get should cause channel closure with BAD_STATE
        let result = proxy.get().await;
        assert!(result.is_err());
        if let Err(fidl::Error::ClientChannelClosed { epitaph, .. }) = result {
            assert_eq!(epitaph, zx::Status::BAD_STATE);
        } else {
            panic!("Expected ClientChannelClosed(BAD_STATE), got {:?}", result);
        }

        let result1 = get_fut1.await;
        assert!(result1.is_err());
    }
}
