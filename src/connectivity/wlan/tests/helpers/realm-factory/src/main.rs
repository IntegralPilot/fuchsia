// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result, bail, format_err};
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_wlan_phy as fidl_wlan_phy;
use fidl_fuchsia_wlan_tap as fidl_wlan_tap;
use fidl_test_wlan_realm::*;
use fuchsia_component::runtime::Dictionary;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, ChildRef, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::{StreamExt, TryStreamExt};
use log::{error, info, warn};
use wlan_fidl_ext::try_unpack::{TryUnpack, WithName};
use zx_status;

struct CreatedRealm {
    realm: RealmInstance,
    wlancfg: Option<ChildRef>,
    wlandevicemonitor: Option<ChildRef>,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: RealmFactoryRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(mut stream: RealmFactoryRequestStream) {
    let mut realms = vec![];
    let result: Result<(), Error> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                RealmFactoryRequest::_UnknownMethod { control_handle, .. } => {
                    control_handle.shutdown_with_epitaph(zx_status::Status::NOT_SUPPORTED);
                    unimplemented!();
                }
                RealmFactoryRequest::CreateRealm { options, dictionary, responder } => {
                    let has_wlan_config = options.wlan_config.is_some();
                    let CreatedRealm { realm, wlancfg, wlandevicemonitor } =
                        create_realm(options).await?;
                    let output_dictionary_handle =
                        realm.root.controller().get_output_dictionary().await?.unwrap();
                    let output_dictionary = Dictionary::from(output_dictionary_handle);
                    output_dictionary.associate_with_handle(dictionary).await;
                    realms.push(realm);

                    let mut response = CreateRealmResponse::default();
                    if has_wlan_config {
                        if let Some(ref wlancfg) = wlancfg {
                            response.wlan_policy_moniker = Some(format!(
                                "./{}",
                                format!("{}", wlancfg).strip_prefix('#').unwrap()
                            ));
                        }
                        let wlandevicemonitor = wlandevicemonitor.unwrap();
                        response.wlandevicemonitor_moniker = Some(format!(
                            "./{}",
                            format!("{}", wlandevicemonitor).strip_prefix('#').unwrap()
                        ));
                    }
                    responder.send(Ok(&response))?;
                }
            }
        }

        Ok(())
    }
    .await;

    if let Err(err) = result {
        // hw-sim tests allow error logs so we panic to ensure test failure.
        panic!("{:?}", err);
    }
}

async fn create_realm(mut options: RealmOptions) -> Result<CreatedRealm, Error> {
    if let Some(topology) = options.topology {
        info!("Building the realm using topology {:#?}", topology);
        let builder = RealmBuilder::new().await?;
        match topology {
            Topology::DriversOnly(config, ..) => {
                builder
                    .driver_test_realm_manifest_setup(
                        fuchsia_driver_test::DRIVER_TEST_REALM_URL,
                        fuchsia_driver_test::Options::new().allow_external_tracing_route(),
                    )
                    .await?;
                setup_trace_manager(
                    &builder,
                    vec![Ref::child(fuchsia_driver_test::COMPONENT_NAME)],
                    TraceManagerHermeticity::Hermetic,
                )
                .await?;
                let realm = builder.build().await?;
                let driver_config = config.driver_config.ok_or_else(|| {
                    format_err!("DriversOnly topology requires driver_config, but none found")
                })?;
                start_and_connect_to_driver_test_realm(&realm, driver_config).await?;
                Ok(CreatedRealm { realm, wlancfg: None, wlandevicemonitor: None })
            }
            TopologyUnknown!() => bail!("Unknown topology"),
        }
    } else if let Some(wlan_config) = options.wlan_config {
        // TODO(b/317255344): Remove this branch when no CTF tests depend on the deprecated API.
        warn!("Building the realm using deprecated wlan_config {:#?}", wlan_config);
        let mut params = RealmBuilderParams::new();
        if let Some(ref name) = wlan_config.name {
            params = params.realm_name(name);
        }
        let builder = RealmBuilder::with_params(params).await?;

        builder
            .driver_test_realm_manifest_setup(
                fuchsia_driver_test::DRIVER_TEST_REALM_URL,
                fuchsia_driver_test::Options::new().allow_external_tracing_route(),
            )
            .await?;

        let dtr_exposes = vec![
            ftest::Capability::Service(ftest::Service {
                name: Some("fuchsia.wlan.phy.Service".to_string()),
                ..Default::default()
            }),
            ftest::Capability::Service(ftest::Service {
                name: Some("fuchsia.wlan.tap.Service".to_string()),
                ..Default::default()
            }),
        ];
        builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await?;

        let WlanComponents { wlancfg, wlandevicemonitor } =
            create_wlan_components(&builder, wlan_config).await?;
        let realm = builder.build().await?;

        // NOTE: We only need devfs to support netdevice-migration.
        if let Some(devfs) = options.devfs_server_end.take() {
            realm.root.get_exposed_dir().open(
                "dev-topological",
                fidl_fuchsia_io::PERM_READABLE | fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY,
                &Default::default(),
                devfs.into_channel(),
            )?;
        }

        Ok(CreatedRealm { realm, wlancfg, wlandevicemonitor: Some(wlandevicemonitor) })
    } else {
        error!("RealmOptions must include either topology or wlan_config: {:#?}", options);
        bail!("RealmOptions missing topology and wlan_config");
    }
}

/// Starts and connects to the driver test realm based on |driver_config|.
async fn start_and_connect_to_driver_test_realm(
    realm: &RealmInstance,
    driver_config: DriverConfig,
) -> Result<()> {
    let start_args = driver_config.driver_test_realm_start_args.ok_or_else(|| {
        format_err!("DriverConfig requires driver_test_realm_start_args, but none found")
    })?;

    realm.driver_test_realm_start(start_args).await?;

    let dev_topological = driver_config
        .dev_topological
        .ok_or_else(|| format_err!("DriverConfig missing dev_topological"))?;
    realm.root.get_exposed_dir().open(
        "dev-topological",
        fidl_fuchsia_io::PERM_READABLE | fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY,
        &Default::default(),
        dev_topological.into_channel(),
    )?;

    let dev_class =
        driver_config.dev_class.ok_or_else(|| format_err!("DriverConfig missing dev_class"))?;
    realm.root.get_exposed_dir().open(
        "dev-class",
        fidl_fuchsia_io::PERM_READABLE | fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY,
        &Default::default(),
        dev_class.into_channel(),
    )?;

    Ok(())
}

// Adds trace_manager to the test realm and routes `fuchsia.tracing.provider.Registry` to
// |tracing_consumers| and `fuchsia.tracing.controller.Controller to the parent component.
async fn setup_trace_manager(
    builder: &RealmBuilder,
    tracing_consumers: Vec<Ref>,
    trace_manager_hermeticity: TraceManagerHermeticity,
) -> Result<(), Error> {
    match trace_manager_hermeticity {
        TraceManagerHermeticity::Hermetic => {
            let trace_manager = builder
                .add_child("trace_manager", "#meta/trace_manager.cm", ChildOptions::new())
                .await?;
            builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol::<
                            fidl_fuchsia_tracing_controller::ProvisionerMarker,
                        >())
                        .from(&trace_manager)
                        .to(Ref::parent()),
                )
                .await?;
            for consumer in tracing_consumers {
                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::protocol::<
                                fidl_fuchsia_tracing_provider::RegistryMarker,
                            >())
                            .from(&trace_manager)
                            .to(consumer),
                    )
                    .await?;
            }
        }
        TraceManagerHermeticity::NonHermetic => {
            for consumer in tracing_consumers {
                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::protocol::<
                                fidl_fuchsia_tracing_provider::RegistryMarker,
                            >())
                            .from(Ref::parent())
                            .to(consumer),
                    )
                    .await?;
            }
        }
    }

    Ok(())
}

struct WlanComponents {
    wlancfg: Option<ChildRef>,
    wlandevicemonitor: ChildRef,
}

async fn create_wlan_components(
    builder: &RealmBuilder,
    config: WlanConfig,
) -> Result<WlanComponents, Error> {
    let (use_legacy_privacy, with_regulatory_region, with_policy) = (
        config.use_legacy_privacy.with_name("use_legacy_privacy"),
        config.with_regulatory_region.with_name("with_regulatory_region"),
        config.with_policy.with_name("with_policy"),
    )
        .try_unpack()?;

    if with_regulatory_region && !with_policy {
        return Err(format_err!(
            "Invalid hw-sim test configuration. The regulatory_region is only intended to be included in tests that include wlancfg too."
        ));
    }

    // Add and configure wlandevicemonitor
    let wlandevicemonitor_options = ChildOptions::new();
    let wlandevicemonitor = builder
        .add_child("wlandevicemonitor", "#meta/wlandevicemonitor.cm", wlandevicemonitor_options)
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fidl_wlan_phy::ServiceMarker>())
                .from(Ref::child(fuchsia_driver_test::COMPONENT_NAME))
                .to(&wlandevicemonitor),
        )
        .await?;

    builder.init_mutable_config_to_empty(&wlandevicemonitor).await?;
    builder
        .set_config_value(&wlandevicemonitor, "wep_supported", use_legacy_privacy.into())
        .await?;
    builder
        .set_config_value(&wlandevicemonitor, "wpa1_supported", use_legacy_privacy.into())
        .await?;

    // Initialize list of tracing consumers that may be appended to if with_policy is true.
    let mut tracing_consumers =
        vec![Ref::child(fuchsia_driver_test::COMPONENT_NAME), (&wlandevicemonitor).into()];

    let wlancfg = if with_policy {
        // Start wlancfg eagerly so that TestHelper can wait for wlancfg to create an interface before
        // allowing each test to proceed. Otherwise, no interface will be created.
        let wlancfg =
            builder.add_child("wlancfg", "#meta/wlancfg.cm", ChildOptions::new().eager()).await?;
        tracing_consumers.push((&wlancfg).into());

        let stash =
            builder.add_child("stash", "#meta/stash_secure.cm", ChildOptions::new()).await?;
        tracing_consumers.push((&stash).into());

        builder.init_mutable_config_to_empty(&wlancfg).await?;
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.wlan.RecoveryProfile".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String("".into())),
            }))
            .await?;
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.wlan.RecoveryEnabled".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(false)),
            }))
            .await?;
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.wlan.RoamingPolicy".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String("".into())),
            }))
            .await?;
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.power.SuspendEnabled".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(false)),
            }))
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.wlan.RecoveryProfile"))
                    .capability(Capability::configuration("fuchsia.wlan.RecoveryEnabled"))
                    .capability(Capability::configuration("fuchsia.wlan.RoamingPolicy"))
                    .capability(Capability::configuration("fuchsia.power.SuspendEnabled"))
                    .from(Ref::self_())
                    .to(&wlancfg),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<
                        fidl_fuchsia_wlan_policy::ClientProviderMarker,
                    >())
                    .capability(Capability::protocol::<
                        fidl_fuchsia_wlan_policy::AccessPointProviderMarker,
                    >())
                    .from(&wlancfg)
                    .to(Ref::parent()),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                    .capability(Capability::protocol::<fidl_fuchsia_inspect::InspectSinkMarker>())
                    .from(Ref::parent())
                    .to(&wlancfg),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::storage("data"))
                    .from(Ref::parent())
                    .to(&stash)
                    .to(&wlancfg),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol::<fidl_fuchsia_wlan_device_service::DeviceMonitorMarker>()
                            .weak(),
                    )
                    .from(&wlandevicemonitor)
                    .to(&wlancfg),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_stash::SecureStoreMarker>())
                    .from(&stash)
                    .to(&wlancfg),
            )
            .await?;

        if with_regulatory_region {
            let regulatory_region = builder
                .add_child("regulatory_region", "#meta/regulatory_region.cm", ChildOptions::new())
                .await?;

            builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::protocol::<
                                fidl_fuchsia_location_namedplace::RegulatoryRegionWatcherMarker,
                            >()
                            .weak(),
                        )
                        .from(&regulatory_region)
                        .to(&wlancfg),
                )
                .await?;

            builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                        .capability(Capability::storage("cache"))
                        .from(Ref::parent())
                        .to(&regulatory_region),
                )
                .await?;
        }

        Some(wlancfg)
    } else {
        None
    };

    // Route capabilities required by the TestHelper in hw-sim tests.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<
                    fidl_fuchsia_wlan_device_service::DeviceMonitorMarker,
                >())
                .from(&wlandevicemonitor)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fidl_fuchsia_sys2::LifecycleControllerMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<fidl_wlan_tap::ServiceMarker>())
                .from(Ref::child(fuchsia_driver_test::COMPONENT_NAME))
                .to(Ref::parent()),
        )
        .await?;

    // Now that all components have been added to tracing_consumers, add and configure
    // a trace_manager component, routing capabilities to collect traces from each components.
    let trace_manager_hermeticity =
        config.trace_manager_hermeticity.unwrap_or(TraceManagerHermeticity::Hermetic);
    setup_trace_manager(&builder, tracing_consumers, trace_manager_hermeticity).await?;

    Ok(WlanComponents { wlancfg, wlandevicemonitor })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_endpoints;
    use fidl_fuchsia_driver_test::RealmArgs;
    use test_case::test_case;

    // RealmOptions without specific topology or wlan_config are invalid
    #[test_case(RealmOptions { ..Default::default() })]
    #[test_case(RealmOptions { topology: None, wlan_config: None, ..Default::default() })]
    #[fuchsia::test]
    async fn reject_invalid_realm_options(opts: RealmOptions) {
        assert!(create_realm(opts).await.is_err());
    }

    // DriversOnly topology with missing or unspecified fields is invalid
    #[test_case(DriversOnly { ..Default::default() })]
    #[test_case(
        DriversOnly {
            driver_config: None,
            ..Default::default()
        }
    )]
    #[test_case(
        DriversOnly {
            driver_config: Some(DriverConfig { ..Default::default() }),
            ..Default::default()
        }
    )]
    #[test_case(
        DriversOnly {
            driver_config: Some(
                DriverConfig {
                    dev_topological: None,
                    dev_class: None,
                    driver_test_realm_start_args: Some(RealmArgs { ..Default::default() }),
                    ..Default::default()
                }
            ),
            ..Default::default()
        }
    )]
    #[fuchsia::test]
    async fn reject_invalid_drivers_only_topology(drivers_only: DriversOnly) {
        let opts = RealmOptions {
            topology: Some(Topology::DriversOnly(drivers_only)),
            ..Default::default()
        };
        assert!(create_realm(opts).await.is_err());
    }

    #[fuchsia::test]
    async fn accept_valid_drivers_only_config() {
        let (_dev_topological_client, dev_topological) = create_endpoints();
        let (_dev_class_client, dev_class) = create_endpoints();
        let opts = RealmOptions {
            topology: Some(Topology::DriversOnly(DriversOnly {
                driver_config: Some(DriverConfig {
                    dev_topological: Some(dev_topological),
                    dev_class: Some(dev_class),
                    driver_test_realm_start_args: Some(RealmArgs { ..Default::default() }),
                    ..Default::default()
                }),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert!(create_realm(opts).await.is_ok());
    }
}
