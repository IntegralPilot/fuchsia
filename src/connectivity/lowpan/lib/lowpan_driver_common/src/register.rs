// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use crate::{Driver, ServeTo, MAX_CONCURRENT};
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_lowpan_driver::{
    DriverMarker, DriverRequest, DriverRequestStream, RegisterProxyInterface,
};
use futures::future::join_all;

/// Registers a driver instance with the given LoWPAN service and returns
/// a future which services requests for the driver.
pub async fn register_and_serve_driver<'a, R, D>(
    name: &str,
    registry: R,
    driver: &'a D,
) -> anyhow::Result<()>
where
    R: RegisterProxyInterface,
    D: Driver + 'a,
{
    let (client_ep, server_ep) = create_endpoints::<DriverMarker>();

    registry.register_device(name, client_ep)?;

    driver.serve_to(server_ep.into_stream()).await?;

    info!("LoWPAN Driver {:?} Stopped.", name);

    Ok(())
}

#[async_trait()]
impl<T: Driver> ServeTo<DriverRequestStream> for T {
    async fn serve_to(&self, request_stream: DriverRequestStream) -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let legacy_joining_protocol_in_use_flag: Arc<AtomicBool> = Arc::default();

        request_stream
            .try_for_each_concurrent(MAX_CONCURRENT, move |cmd| {
                match cmd {
                    DriverRequest::GetProtocols { protocols, .. } => {
                        macro_rules! handle_protocol {
                            ($futures:expr, $protocol:expr) => {
                                if let Some(server_end) = $protocol {
                                    $futures.push(self.serve_to(server_end.into_stream()));
                                }
                            };
                        }

                        let mut futures = vec![];

                        handle_protocol!(futures, protocols.capabilities);
                        handle_protocol!(futures, protocols.device);
                        handle_protocol!(futures, protocols.device_extra);
                        handle_protocol!(futures, protocols.experimental_device);
                        handle_protocol!(futures, protocols.experimental_device_extra);
                        handle_protocol!(futures, protocols.telemetry_provider);
                        handle_protocol!(futures, protocols.energy_scan);
                        handle_protocol!(futures, protocols.meshcop);
                        handle_protocol!(futures, protocols.counters);
                        handle_protocol!(futures, protocols.device_test);
                        handle_protocol!(futures, protocols.device_route);
                        handle_protocol!(futures, protocols.device_route_extra);
                        handle_protocol!(futures, protocols.thread_dataset);
                        handle_protocol!(futures, protocols.thread_feature);
                        handle_protocol!(futures, protocols.epskc);

                        if let Some(server_end) = protocols.thread_legacy_joining {
                            let stream = server_end.into_stream();
                            // We only let there be one outstanding instance of this protocol.
                            if !legacy_joining_protocol_in_use_flag
                                .swap(true, Ordering::Relaxed)
                            {
                                info!("Mutually exclusive thread_legacy_joining channel requested and vended.");
                                let flag = legacy_joining_protocol_in_use_flag.clone();
                                futures.push(
                                    self.serve_to(stream)
                                        .inspect(move |_| {
                                            info!("thread_legacy_joining channel released.");
                                            flag.store(false, Ordering::Relaxed)
                                        })
                                        .boxed(),
                                );
                            } else {
                                warn!("Cannot vend thread_legacy_joining, one instance already outstanding.");
                            }
                        }

                        join_all(futures).map(|_| Ok(()))
                    }
                }
            })
            .await?;

        Ok(())
    }
}

/// Registers a driver instance with the given LoWPAN service factory registrar
/// and returns a future which services factory requests for the driver.
pub async fn register_and_serve_driver_factory<'a, R, D>(
    name: &str,
    registry: R,
    driver: &'a D,
) -> anyhow::Result<()>
where
    R: fidl_fuchsia_factory_lowpan::FactoryRegisterProxyInterface,
    D: Driver + 'a,
{
    use fidl_fuchsia_factory_lowpan::{FactoryDriverMarker, FactoryDriverRequest};

    let (client_ep, server_ep) = create_endpoints::<FactoryDriverMarker>();

    registry.register(name, client_ep)?;

    server_ep
        .into_stream()
        .try_for_each_concurrent(MAX_CONCURRENT, |cmd| async {
            match cmd {
                FactoryDriverRequest::GetFactoryDevice { device_factory, .. } => {
                    let stream = device_factory.into_stream();
                    let _ = driver.serve_to(stream).await;
                }
            }
            Ok(())
        })
        .await?;

    info!("LoWPAN FactoryDriver {:?} Stopped.", name);

    Ok(())
}
