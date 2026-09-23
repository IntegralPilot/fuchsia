// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod device;
mod device_watch;
mod inspect;
mod phy_event_service;
mod service;
mod watchable_map;
mod watcher_service;

use anyhow::Error;
use fidl_fuchsia_wlan_device_service as fidl_svc;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::{Inspector, InspectorConfig};
use futures::channel::mpsc;
use futures::future::{BoxFuture, try_join5};
use futures::{StreamExt, TryFutureExt, TryStreamExt};
use log::{error, info};
use std::sync::Arc;

fn serve_phys(
    phys: Arc<device::PhyMap>,
    inspect_tree: Arc<inspect::WlanMonitorTree>,
    phy_event_sender: mpsc::Sender<(u16, device::PhyEvent)>,
) -> BoxFuture<'static, Result<std::convert::Infallible, Error>> {
    info!("Serving real device environment");
    let fut = device::serve_phys(phys, inspect_tree, phy_event_sender);
    Box::pin(fut)
}

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    diagnostics_log::initialize(
        diagnostics_log::PublishOptions::default()
            .tags(&["wlan"])
            .enable_metatag(diagnostics_log::Metatag::Target),
    )?;
    info!("Starting");

    let (phy_event_sender, phy_event_receiver) = mpsc::channel(5);
    let (phy_event_service, phy_event_fut) =
        phy_event_service::serve_phy_events(phy_event_receiver);
    let (phys, phy_events) = device::PhyMap::new();
    let phys = Arc::new(phys);
    let (ifaces, iface_events) = device::IfaceMap::new();
    let ifaces = Arc::new(ifaces);

    let (watcher_service, watcher_fut) =
        watcher_service::serve_watchers(phys.clone(), ifaces.clone(), phy_events, iface_events);

    let inspector = Inspector::new(InspectorConfig::default().size(inspect::VMO_SIZE_BYTES));
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());
    let cfg = wlandevicemonitor_config::Config::take_from_startup_handle();
    inspector.root().record_child("config", |config_node| cfg.record_inspect(config_node));
    let ifaces_tree = Arc::new(inspect::IfacesTree::new(inspector.clone()));
    let inspect_tree = Arc::new(inspect::WlanMonitorTree::new(inspector));

    let phy_server = serve_phys(phys.clone(), inspect_tree.clone(), phy_event_sender);

    let iface_counter = Arc::new(service::IfaceCounter::new());

    let (new_iface_sink, new_iface_stream) = mpsc::unbounded();

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(fidl_svc::DeviceMonitorRequestStream::from);
    fs.take_and_serve_directory_handle()?;

    // Arbitrarily limit number of clients to 1000. In practice, it's usually one or two.
    // TODO(https://fxbug.dev/382306025): While the wlandevicemonitor component can support many
    // clients, we could simplify the design of the component by serving only one client at a time.
    const MAX_CONCURRENT: usize = 1000;
    let ifaces_wrapper = device::IfaceWrapper::new(&ifaces, &ifaces_tree);
    let fidl_fut = fs.map(Ok).try_for_each_concurrent(MAX_CONCURRENT, {
        // Rebind these variables to borrows so the borrows can be moved into the FnMut.
        let phys = &phys;
        let ifaces_wrapper = &ifaces_wrapper;
        let watcher_service = &watcher_service;
        let phy_event_service = &phy_event_service;
        let new_iface_sink = &new_iface_sink;
        let iface_counter = &iface_counter;
        let cfg = &cfg;
        move |mut s: fidl_svc::DeviceMonitorRequestStream| async move {
            while let Ok(Some(request)) = s.try_next().await {
                service::handle_monitor_request(
                    request,
                    phys,
                    ifaces_wrapper,
                    watcher_service,
                    phy_event_service,
                    new_iface_sink,
                    iface_counter,
                    cfg,
                )
                .await?
            }
            Ok(())
        }
    });

    let new_iface_fut = service::handle_new_iface_stream(&phys, &ifaces_wrapper, new_iface_stream);

    let ((), (), (), (), ()) = try_join5(
        fidl_fut,
        phy_server.map_ok(|_: std::convert::Infallible| ()),
        watcher_fut.map_ok(|_: std::convert::Infallible| ()),
        phy_event_fut.map_ok(|_: std::convert::Infallible| ()),
        new_iface_fut,
    )
    .await?;
    error!("Exiting");
    // wlandevicemonitor should never exit. Return an error to trigger device reboot via the
    // component's `on_terminate: "reboot"` setting.
    Err(anyhow::anyhow!("Exiting"))
}
