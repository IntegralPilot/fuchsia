// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::emulator::EMULATOR_ROOT_DRIVER_URL;
use anyhow::{Error, format_err};
use fidl_fuchsia_bluetooth_bredr as fbredr;
use fidl_fuchsia_bluetooth_gatt as fbgatt;
use fidl_fuchsia_bluetooth_le as fble;
use fidl_fuchsia_bluetooth_snoop::SnoopMarker;
use fidl_fuchsia_bluetooth_sys as fbsys;
use fidl_fuchsia_device::NameProviderMarker;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_hardware_bluetooth as fhbt;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger::LogSinkMarker;
use fidl_fuchsia_stash::SecureStoreMarker;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route, ScopedInstance,
};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::FutureExt;
use realmbuilder_mock_helpers::stateless_mock_responder;

pub const SHARED_STATE_INDEX: &str = "BT-CORE-REALM";
pub const DEFAULT_TEST_DEVICE_NAME: &str = "fuchsia-bt-integration-test";

// Use relative URLs because the library `deps` on all of these components, so any
// components that depend (even transitively) on CoreRealm will include these components in
// their package.
mod constants {
    pub mod bt_init {
        pub const URL: &str = "#meta/test-bt-init.cm";
        pub const MONIKER: &str = "bt-init";
    }
    pub mod secure_stash {
        pub const URL: &str = "#meta/test-stash-secure.cm";
        pub const MONIKER: &str = "secure-stash";
    }
    pub mod mock_name_provider {
        pub const MONIKER: &str = "mock-name-provider";
    }
    pub mod mock_snoop {
        pub const MONIKER: &str = "mock-snoop";
    }
}

/// The CoreRealm represents a hermetic, fully-functional instance of the Fuchsia Bluetooth core
/// stack, complete with all components (bt-init, bt-gap, bt-host, bt-rfcomm) and a bt-hci
/// emulator. Clients should use the `create` method to construct an instance, and the `instance`
/// method to access the various production capabilities and test interfaces (e.g. from the bt-hci
/// emulator) exposed from the core stack. Clients of the CoreRealm must offer the `tmp` storage
/// capability from the test manager to the "#realm_builder" underlying the RealmInstance.
pub struct CoreRealm {
    realm: RealmInstance,
}

impl CoreRealm {
    pub async fn create(test_component: String) -> Result<Self, Error> {
        // We need to resolve our test component manually. Eventually component framework could provide
        // an introspection way of resolving your own component.
        let resolved_test_component = {
            let client = fuchsia_component::client::connect_to_protocol_at_path::<
                fidl_fuchsia_component_resolution::ResolverMarker,
            >("/svc/fuchsia.component.resolution.Resolver-hermetic")
            .unwrap();
            client
                .resolve(test_component.as_str())
                .await
                .unwrap()
                .expect("Failed to resolve test component")
        };

        let builder = RealmBuilder::new().await?;
        let _ = builder.driver_test_realm_setup().await?;

        // Create the components within CoreRealm
        let bt_init = builder
            .add_child(
                constants::bt_init::MONIKER,
                constants::bt_init::URL,
                ChildOptions::new().eager(),
            )
            .await?;
        let secure_stash = builder
            .add_child(
                constants::secure_stash::MONIKER,
                constants::secure_stash::URL,
                ChildOptions::new(),
            )
            .await?;
        let mock_name_provider = builder
            .add_local_child(
                constants::mock_name_provider::MONIKER,
                |handles| {
                    stateless_mock_responder::<NameProviderMarker, _>(handles, |req| {
                        let responder = req
                            .into_get_device_name()
                            .ok_or_else(|| format_err!("got unexpected NameProviderRequest"))?;
                        Ok(responder.send(Ok(DEFAULT_TEST_DEVICE_NAME))?)
                    })
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await?;
        let mock_snoop = builder
            .add_local_child(
                constants::mock_snoop::MONIKER,
                |handles| {
                    stateless_mock_responder::<SnoopMarker, _>(handles, |req| {
                        // just drop the request, should be sufficient
                        let _ = req
                            .into_start()
                            .ok_or_else(|| format_err!("got unexpected SnoopRequest"))?;
                        Ok(())
                    })
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await?;

        // Add capability routing between components within CoreRealm
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<LogSinkMarker>())
                    .capability(Capability::dictionary("diagnostics"))
                    .from(Ref::parent())
                    .to(&bt_init)
                    .to(&secure_stash),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::storage("tmp"))
                    .from(Ref::parent())
                    .to(&secure_stash),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<SecureStoreMarker>())
                    .from(&secure_stash)
                    .to(&bt_init),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<NameProviderMarker>())
                    .from(&mock_name_provider)
                    .to(&bt_init),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<SnoopMarker>())
                    .from(&mock_snoop)
                    .to(&bt_init),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fbgatt::Server_Marker>())
                    .capability(Capability::protocol::<fble::CentralMarker>())
                    .capability(Capability::protocol::<fble::PeripheralMarker>())
                    .capability(Capability::protocol::<fbsys::AccessMarker>())
                    .capability(Capability::protocol::<fbsys::HostWatcherMarker>())
                    .capability(Capability::protocol::<fbredr::ProfileMarker>())
                    .capability(Capability::protocol::<fbsys::BootstrapMarker>())
                    .from(&bt_init)
                    .to(Ref::parent()),
            )
            .await?;

        // Add the `fuchsia.bluetooth.FastPairProvider` capability to the realm with its value
        // set to false as all Core Realm tests do not require it.
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.bluetooth.FastPairProvider".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(false)),
            }))
            .await?;
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.bluetooth.Rfcomm".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(true)),
            }))
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.bluetooth.FastPairProvider"))
                    .capability(Capability::configuration("fuchsia.bluetooth.Rfcomm"))
                    .from(Ref::self_())
                    .to(&bt_init),
            )
            .await?;

        let dtr_exposes = vec![Capability::service::<fhbt::ServiceMarker>().into()];
        let _ = builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::service::<fhbt::ServiceMarker>())
                    .from(Ref::child(fuchsia_driver_test::COMPONENT_NAME))
                    .to(&bt_init),
            )
            .await?;

        crate::host_realm::add_host_routes(&builder, &bt_init).await?;
        let instance = builder.build().await?;

        // Start DriverTestRealm
        let args = fdt::RealmArgs {
            root_driver: Some(EMULATOR_ROOT_DRIVER_URL.to_string()),
            software_devices: Some(vec![fidl_fuchsia_driver_test::SoftwareDevice {
                device_name: "bt-hci-emulator".to_string(),
                device_id: bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_BT_HCI_EMULATOR,
            }]),
            test_component: Some(resolved_test_component),
            dtr_exposes: Some(dtr_exposes),
            ..Default::default()
        };
        instance.driver_test_realm_start(args).await?;

        Ok(Self { realm: instance })
    }

    pub fn instance(&self) -> &ScopedInstance {
        &self.realm.root
    }

    pub fn dev(&self) -> Result<fio::DirectoryProxy, Error> {
        self.realm.driver_test_realm_connect_to_dev()
    }
}
