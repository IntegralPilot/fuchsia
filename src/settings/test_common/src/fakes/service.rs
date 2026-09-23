// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};

use futures::future::LocalBoxFuture;
use futures::lock::Mutex;
use settings_common::service_context::GenerateService;
use std::rc::Rc;

/// Trait for providing a service.
pub trait Service {
    /// Returns true if this service can process the given service name, false
    /// otherwise.
    fn can_handle_service(&self, service_name: &str) -> bool;

    /// Processes the request stream within the specified channel. Ok is returned
    /// on success, an error otherwise.
    fn process_stream(&mut self, service_name: &str, channel: zx::Channel) -> Result<(), Error>;
}

pub type ServiceRegistryHandle = Rc<Mutex<ServiceRegistry>>;

/// A helper class that gathers services through registration and directs
/// the appropriate channels to them.
pub struct ServiceRegistry {
    services: Vec<Rc<Mutex<dyn Service>>>,
}

impl ServiceRegistry {
    pub fn create() -> ServiceRegistryHandle {
        Rc::new(Mutex::new(ServiceRegistry { services: Vec::new() }))
    }

    pub fn register_service(&mut self, service: Rc<Mutex<dyn Service>>) {
        self.services.push(service);
    }

    async fn service_channel(&self, service_name: &str, channel: zx::Channel) -> Result<(), Error> {
        for service_handle in self.services.iter() {
            let mut service = service_handle.lock().await;
            if service.can_handle_service(service_name) {
                return service.process_stream(service_name, channel);
            }
        }

        Err(format_err!("channel not handled for service: {}", service_name))
    }

    pub fn serve(registry_handle: ServiceRegistryHandle) -> GenerateService {
        Box::new(
            move |service_name: &str,
                  channel: zx::Channel|
                  -> LocalBoxFuture<'_, Result<(), Error>> {
                let registry_handle_clone = registry_handle.clone();
                let service_name_clone = String::from(service_name);

                Box::pin(async move {
                    registry_handle_clone
                        .lock()
                        .await
                        .service_channel(service_name_clone.as_str(), channel)
                        .await
                })
            },
        )
    }
}
