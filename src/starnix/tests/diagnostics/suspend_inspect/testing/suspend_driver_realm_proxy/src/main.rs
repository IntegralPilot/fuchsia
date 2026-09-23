// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, RealmBuilder, RealmInstance, Ref};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures_util::StreamExt;
use std::cell::RefCell;
use std::rc::Rc;
use {
    fidl_fuchsia_component_test as ffct, fidl_fuchsia_driver_test as fdt,
    fidl_fuchsia_hardware_power_suspend as ffhs, fidl_fuchsia_kernel as ffk,
    fidl_fuchsia_test_suspend as fftsu, fidl_fuchsia_test_syscalls as ffts,
    fuchsia_async as fasync,
};

enum IncomingService {
    SuspendRealm(fftsu::RealmRequestStream),
}

async fn setup_driver_realm_builder() -> Result<RealmBuilder> {
    // TODO: 296625903 - `fuchsia.kernel.CpuResource` is taken from the test parent, but should be
    // initialized here instead.

    let realm_builder = RealmBuilder::new().await?;
    // This initializes static routes from a fixed textual manifest.
    realm_builder.driver_test_realm_setup().await.context("while adding the static routes")?;

    realm_builder
        .driver_test_realm_add_dtr_offers(
            &vec![Capability::protocol::<ffk::CpuResourceMarker>().into()],
            Ref::parent(),
        )
        .await
        .context("while offering needed driver test realm capabilities")?;

    // These are the services exposed by the driver test realm in devfs.
    // Extract them for use "here".
    realm_builder
        .driver_test_realm_add_dtr_exposes(&vec![
            Capability::service::<ffhs::SuspendServiceMarker>().into(),
            Capability::service::<ffts::ControlServiceMarker>().into(),
        ])
        .await
        .context("while exposing needed driver test realm capabilities")?;
    Ok(realm_builder)
}

async fn create_realm() -> Result<RealmInstance> {
    let realm_builder =
        setup_driver_realm_builder().await.context("while setting up realm builder")?;
    let driver_realm = realm_builder.build().await.context("while building realm")?;
    driver_realm
        .driver_test_realm_start(fdt::RealmArgs {
            // Offered from driver's internal test_realm to drivers.
            dtr_offers: Some(vec![
                // Offering by name string to avoid needing to add these as a dependency
                // just for the sake of `PROTOCOL_NAME`.
                ffct::Capability::Protocol(ffct::Protocol {
                    name: Some("fuchsia.kernel.CpuResource".into()),
                    ..ffct::Protocol::default()
                }),
                ffct::Capability::Protocol(ffct::Protocol {
                    name: Some("fuchsia.boot.WriteOnlyLog".into()),
                    ..ffct::Protocol::default()
                }),
            ]),
            // Exposed from driver's internal test_realm to driver_test_realm.
            dtr_exposes: Some(vec![
                Capability::service::<ffhs::SuspendServiceMarker>().into(),
                Capability::service::<ffts::ControlServiceMarker>().into(),
            ]),
            ..Default::default()
        })
        .await
        .context("while starting driver realm")?;
    Ok(driver_realm)
}

struct RealmServer {
    // The singleton test realm. Repeated calls to Create will shut down this
    // realm (if any) and remove others.
    realm: RefCell<Option<RealmInstance>>,
}

impl RealmServer {
    fn new() -> Rc<Self> {
        Rc::new(Self { realm: RefCell::new(None) })
    }

    // Replaces the currently active realm with an optional new one. Replacing
    // with `None` is the same as shutting the realm down.
    fn replace_realm(self: &Rc<Self>, realm: Option<RealmInstance>) {
        let mut r = self.realm.borrow_mut();
        if r.is_some() && realm.is_some() {
            log::warn!(concat!(
                "a realm was previously created. ",
                "That realm will be shut down, and a new one created. ",
                "Make sure this is what you wanted."
            ));
        }
        *r = realm;
    }

    async fn serve_realm_factory(self: Rc<Self>, service: IncomingService) {
        match service {
            IncomingService::SuspendRealm(stream) => {
                self.clone().serve_suspend_realm(stream).await;
            }
        };
    }

    async fn serve_suspend_realm(self: Rc<Self>, mut stream: fftsu::RealmRequestStream) {
        // Realm will be kept alive for as long as the loop below spins.
        fasync::Task::local(async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(fftsu::RealmRequest::Create { responder, .. }) => {
                        let realm = create_realm().await.expect("failed to build the test realm");
                        self.replace_realm(Some(realm));
                        log::debug!("Test realm created, proceeding.");
                        let _ = responder.send().unwrap();
                    }
                    Err(err) => {
                        log::error!("error while serving Realm: {:?}", &err);
                    }
                }
            }
            log::debug!("no longer serving the suspend realm, realm will be torn down now.");
            self.replace_realm(None);
        })
        .detach();
    }
}

#[fuchsia::main(logging_tags = ["suspend_driver_realm_proxy", "test"])]
async fn main() -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(IncomingService::SuspendRealm);
    fs.take_and_serve_directory_handle()?;

    // Maybe simplify.
    let server = RealmServer::new();
    fs.for_each(|stream| async {
        server.clone().serve_realm_factory(stream).await;
    })
    .await;
    log::debug!("realm proxy shutting down. presumably the realm has been torn down.");
    Ok(())
}
