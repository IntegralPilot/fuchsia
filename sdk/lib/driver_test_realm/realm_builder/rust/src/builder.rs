// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use cm_rust::FidlIntoNative;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_io as fio;
use flyweights::FlyStr;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, ChildRef, CollectionRef, LocalComponentHandles, RealmBuilder,
    RealmInstance, Ref, Route,
};
use futures::{StreamExt, TryStreamExt};
use std::sync::Arc;
use zx::AsHandleRef;

fn clone(
    dir: &fio::DirectoryProxy,
) -> Result<fidl::endpoints::ClientEnd<fio::DirectoryMarker>, fidl::Error> {
    let (client_end, server_end) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
    dir.clone(fidl::endpoints::ServerEnd::new(server_end.into_channel()))?;
    Ok(client_end)
}

async fn internal_serve(
    stream: fidl_fuchsia_driver_test::InternalRequestStream,
    test_pkg_dir: Arc<fio::DirectoryProxy>,
    test_resolution_context: Arc<Option<fidl_fuchsia_component_resolution::Context>>,
    boot_dir: Arc<Option<fio::DirectoryProxy>>,
    boot_driver_components: Arc<Option<Vec<String>>>,
) {
    stream
        .try_for_each_concurrent(None, |request| {
            let test_pkg_dir = test_pkg_dir.clone();
            let test_resolution_context = test_resolution_context.clone();
            let boot_dir = boot_dir.clone();
            let boot_driver_components = boot_driver_components.clone();
            async move {
                match request {
                    fidl_fuchsia_driver_test::InternalRequest::GetTestPackage { responder } => {
                        let cloned = clone(test_pkg_dir.as_ref())?;
                        responder.send(Ok(Some(cloned)))?;
                    }
                    fidl_fuchsia_driver_test::InternalRequest::GetTestResolutionContext {
                        responder,
                    } => responder.send(Ok(test_resolution_context.as_ref().as_ref()))?,
                    fidl_fuchsia_driver_test::InternalRequest::GetBootDirectory { responder } => {
                        match boot_dir.as_ref() {
                            Some(boot_dir) => {
                                let cloned = clone(boot_dir)?;
                                responder.send(Ok(Some(cloned)))?;
                            }
                            None => responder.send(Ok(None))?,
                        }
                    }
                    fidl_fuchsia_driver_test::InternalRequest::GetBootDriverOverrides {
                        responder,
                    } => responder.send(Ok(boot_driver_components
                        .as_ref()
                        .clone()
                        .unwrap_or(vec![])
                        .as_slice()))?,
                };
                Ok(())
            }
        })
        .await
        .expect("fuchsia.driver.test.Internal failed.");
}

async fn resource_provider_serve(
    stream: fidl_fuchsia_driver_test::ResourceProviderRequestStream,
    devicetree: Arc<Option<zx::Vmo>>,
) {
    stream
        .try_for_each_concurrent(None, |request| {
            let devicetree = devicetree.clone();
            async move {
                match request {
                    fidl_fuchsia_driver_test::ResourceProviderRequest::GetDeviceTree {
                        responder,
                    } => {
                        if let Some(vmo) = devicetree.as_ref().as_ref().map(|d| {
                            d.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicate")
                        }) {
                            responder.send(Ok(vmo))?;
                        } else {
                            responder.send(Err(zx::Status::NOT_FOUND.into_raw()))?;
                        }
                    }
                };
                Ok(())
            }
        })
        .await
        .expect("fuchsia.driver.test.Internal failed.");
}

async fn run_internal_server(
    handles: LocalComponentHandles,
    test_pkg_dir: Arc<fio::DirectoryProxy>,
    test_resolution_context: Arc<Option<fidl_fuchsia_component_resolution::Context>>,
    boot_dir: Arc<Option<fio::DirectoryProxy>>,
    boot_driver_components: Arc<Option<Vec<String>>>,
    devicetree: Arc<Option<zx::Vmo>>,
) -> Result<()> {
    let mut fs = ServiceFs::new();

    fs.dir("svc").add_fidl_service(
        move |stream: fidl_fuchsia_driver_test::InternalRequestStream| {
            fasync::Task::spawn(internal_serve(
                stream,
                test_pkg_dir.clone(),
                test_resolution_context.clone(),
                boot_dir.clone(),
                boot_driver_components.clone(),
            ))
            .detach();
        },
    );
    fs.dir("svc").add_fidl_service(
        move |stream: fidl_fuchsia_driver_test::ResourceProviderRequestStream| {
            fasync::Task::spawn(resource_provider_serve(stream, devicetree.clone())).detach();
        },
    );
    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

// Basic equality check of just the name.
fn capabilities_eq_name(a: &ftest::Capability, b: &ftest::Capability) -> bool {
    match (a, b) {
        (ftest::Capability::Protocol(a), ftest::Capability::Protocol(b)) => a.name == b.name,
        (ftest::Capability::Directory(a), ftest::Capability::Directory(b)) => a.name == b.name,
        (ftest::Capability::Storage(a), ftest::Capability::Storage(b)) => a.name == b.name,
        (ftest::Capability::Service(a), ftest::Capability::Service(b)) => a.name == b.name,
        (ftest::Capability::EventStream(a), ftest::Capability::EventStream(b)) => a.name == b.name,
        (ftest::Capability::Config(a), ftest::Capability::Config(b)) => a.name == b.name,
        (ftest::Capability::Dictionary(a), ftest::Capability::Dictionary(b)) => a.name == b.name,
        (ftest::Capability::Resolver(a), ftest::Capability::Resolver(b)) => a.name == b.name,
        (ftest::Capability::Runner(a), ftest::Capability::Runner(b)) => a.name == b.name,
        _ => false,
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    using_subpackage: Option<bool>,
    driver_offers: Option<(Ref, Vec<ftest::Capability>)>,
    driver_exposes: Option<Vec<ftest::Capability>>,
    extra_realm_capabilities: Vec<(ftest::Capability, Ref)>,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn using_subpackage(mut self, using_subpackage: bool) -> Self {
        self.using_subpackage = Some(using_subpackage);
        self
    }

    pub fn driver_offers(mut self, provider: Ref, offers: Vec<ftest::Capability>) -> Self {
        self.driver_offers = Some((provider, offers));
        self
    }

    pub fn driver_exposes(mut self, exposes: Vec<ftest::Capability>) -> Self {
        self.driver_exposes = Some(exposes);
        self
    }

    pub fn add_extra_realm_capability(
        mut self,
        capability: ftest::Capability,
        from: impl Into<Ref>,
    ) -> Self {
        self.extra_realm_capabilities.push((capability, from.into()));
        self
    }
}

#[async_trait::async_trait]
pub trait DriverTestRealmBuilder {
    /// Set up the driver test realm.
    /// This version is a collapsed version compared to the previous version which used a two layer
    /// realm builder setup.
    async fn driver_test_realm_setup(
        &self,
        options: Options,
        args: fdt::RealmArgs,
    ) -> Result<&Self>;
}

#[async_trait::async_trait]
impl DriverTestRealmBuilder for RealmBuilder {
    async fn driver_test_realm_setup(
        &self,
        options: Options,
        args: fdt::RealmArgs,
    ) -> Result<&Self> {
        let manifest_provider =
            fuchsia_component::client::connect_to_protocol::<fdt::ManifestProviderMarker>()?;
        let stream = manifest_provider
            .get_manifest(&fdt::GetManifestRequest {
                using_subpackage: options.using_subpackage,
                ..Default::default()
            })
            .await?
            .expect("manifest stream");

        let mut manifest: Vec<u8> = vec![];
        loop {
            let mut read = stream.read_to_vec(
                zx::StreamReadOptions::empty(),
                fidl_fuchsia_io::MAX_TRANSFER_SIZE as usize,
            )?;
            if read.is_empty() {
                break;
            }
            manifest.append(&mut read);
        }

        let component = fidl::unpersist::<fdecl::Component>(manifest.as_slice())
            .context("unpersisting the manifest vector")?;

        // Keep the rust and c++ realm_builder setups in sync.
        // LINT.IfChange
        let realm = self
            .add_child_realm_from_decl(
                "driver_test_realm",
                component.fidl_into_native(),
                ChildOptions::new(),
            )
            .await?;

        // From the test root into the dtr.
        self.add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.diagnostics.ArchiveAccessor"))
                .from(Ref::parent())
                .to(&realm),
        )
        .await?;

        let dtr_support: ChildRef = "dtr_support".into();
        let fake_resolver: ChildRef = "fake_resolver".into();
        let driver_manager: ChildRef = "driver_manager".into();
        let driver_index: ChildRef = "driver_index".into();

        let boot_drivers: CollectionRef = "boot-drivers".into();
        let base_drivers: CollectionRef = "base-drivers".into();
        let full_drivers: CollectionRef = "full-drivers".into();

        let devicetree = Arc::new(args.devicetree);

        // Get the test component information.
        let test_component = if let Some(test_component) = args.test_component {
            test_component
        } else {
            let realm = fuchsia_component::client::connect_to_protocol::<
                fidl_fuchsia_component::RealmMarker,
            >()?;

            realm.get_resolved_info().await.unwrap().unwrap()
        };

        let test_resolution_context = Arc::new(test_component.resolution_context);
        let test_pkg_dir = Arc::new(
            test_component.package.expect("a pkg").directory.expect("a directory").into_proxy(),
        );

        let boot_dir = Arc::new(match args.boot {
            Some(boot_dir) => {
                if !boot_dir.as_handle_ref().is_invalid() {
                    Some(boot_dir.into_proxy())
                } else {
                    None
                }
            }
            None => None,
        });
        let boot_driver_components = Arc::new(args.boot_driver_components);

        // Route the resolvers from the test root.
        self.add_route(
            Route::new()
                .capability(
                    Capability::protocol_by_name("fuchsia.component.resolution.Resolver-hermetic")
                        .optional(),
                )
                .capability(
                    Capability::protocol_by_name("fuchsia.pkg.PackageResolver-hermetic").optional(),
                )
                .from(Ref::parent())
                .to(&realm),
        )
        .await?;

        // Setup the local Internal protocol server child.
        let driver_test_internal = realm
            .add_local_child(
                "driver_test_internal",
                move |handles| {
                    let test_pkg_dir = test_pkg_dir.clone();
                    let test_resolution_context = test_resolution_context.clone();
                    let boot_dir = boot_dir.clone();
                    let boot_driver_components = boot_driver_components.clone();
                    let devicetree = devicetree.clone();

                    Box::pin(run_internal_server(
                        handles,
                        test_pkg_dir,
                        test_resolution_context,
                        boot_dir,
                        boot_driver_components,
                        devicetree,
                    ))
                },
                ChildOptions::new(),
            )
            .await?;

        // These are capabilities that are routed from void by default but can be provided manually
        // from the user through extra_realm_capabilities.
        let mut tunnel_boot_items = false;
        let mut voided_offers: Vec<ftest::Capability> = vec![
            Capability::protocol_by_name("fuchsia.tracing.provider.Registry").optional().into(),
            Capability::protocol_by_name("fuchsia.boot.WriteOnlyLog").optional().into(),
            Capability::protocol_by_name("fuchsia.scheduler.RoleManager").optional().into(),
            Capability::protocol_by_name("fuchsia.boot.Items").optional().into(),
            Capability::protocol_by_name("fuchsia.kernel.IommuResource").optional().into(),
            Capability::protocol_by_name("fuchsia.diagnostics.LogFlusher").optional().into(),
            Capability::protocol_by_name("fuchsia.kernel.MexecResource").optional().into(),
            Capability::protocol_by_name("fuchsia.kernel.PowerResource").optional().into(),
        ];
        for (capability, from) in options.extra_realm_capabilities {
            // Remove the default voiding for any user provided capabilities.
            voided_offers.retain(|voided| !capabilities_eq_name(voided, &capability));

            if from != Ref::void()
                && capabilities_eq_name(
                    &Capability::protocol_by_name("fuchsia.boot.Items").into(),
                    &capability,
                )
            {
                tunnel_boot_items = true;
            }

            self.add_route(Route::new().capability(capability).from(from).to(&realm)).await?;
        }

        // Set the default void route for remaining voided offers.
        for voided in voided_offers {
            self.add_route(Route::new().capability(voided).from(Ref::void()).to(&realm)).await?;
        }

        // Provide offers from the driver_offers, if the test provides one, to the driver
        // collections.
        if args.dtr_offers.is_some() {
            panic!("Please use |Options::driver_offers| instead of dtr_offers.")
        }
        if let Some((provider, offers)) = options.driver_offers {
            for offer in offers {
                self.add_route(
                    Route::new().capability(offer.clone()).from(provider.clone()).to(&realm),
                )
                .await?;
                realm
                    .add_route(
                        Route::new()
                            .capability(offer)
                            .from(Ref::parent())
                            .to(&boot_drivers)
                            .to(&base_drivers)
                            .to(&full_drivers),
                    )
                    .await?;
            }
        }

        // Provide exposes from the driver collections to the test.
        if args.dtr_exposes.is_some() {
            panic!("Please use |Options::driver_exposes| instead of dtr_exposes.")
        }
        if let Some(exposes) = options.driver_exposes {
            for expose in exposes {
                realm
                    .add_route(
                        Route::new()
                            .capability(expose.clone())
                            .from(&boot_drivers)
                            .to(Ref::parent()),
                    )
                    .await?;
                realm
                    .add_route(
                        Route::new()
                            .capability(expose.clone())
                            .from(&base_drivers)
                            .to(Ref::parent()),
                    )
                    .await?;
                realm
                    .add_route(
                        Route::new()
                            .capability(expose.clone())
                            .from(&full_drivers)
                            .to(Ref::parent()),
                    )
                    .await?;

                self.add_route(Route::new().capability(expose).from(&realm).to(Ref::parent()))
                    .await?;
            }
        }

        // Setup the driver test resource provider.
        realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name(
                        "fuchsia.driver.test.ResourceProvider",
                    ))
                    .from(&driver_test_internal)
                    .to(&dtr_support),
            )
            .await?;

        // Setup various basic config capabilities.
        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.testrealm.TunnelBootItems".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(
                    tunnel_boot_items,
                )),
            }))
            .await?;

        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.testrealm.BoardName".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    args.board_name.unwrap_or_default().into(),
                )),
            }))
            .await?;

        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.testrealm.PlatformVid".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    args.platform_vid.map(|v| v.to_string()).unwrap_or_default().into(),
                )),
            }))
            .await?;

        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.testrealm.PlatformPid".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    args.platform_pid.map(|v| v.to_string()).unwrap_or_default().into(),
                )),
            }))
            .await?;

        let bind_eager = args.driver_bind_eager.unwrap_or_default();
        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.BindEager".parse()?,
                value: cm_rust::ConfigValue::Vector(cm_rust::ConfigVectorValue::StringVector(
                    bind_eager.into_iter().map(FlyStr::new).collect::<Box<[_]>>(),
                )),
            }))
            .await?;

        let driver_disable = args.driver_disable.unwrap_or_default();
        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.DisabledDrivers".parse()?,
                value: cm_rust::ConfigValue::Vector(cm_rust::ConfigVectorValue::StringVector(
                    driver_disable.into_iter().map(FlyStr::new).collect::<Box<[_]>>(),
                )),
            }))
            .await?;

        let driver_index_stop_timeout_millis = args.driver_index_stop_timeout_millis.unwrap_or(-1);
        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.index.StopOnIdleTimeoutMillis".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Int64(
                    driver_index_stop_timeout_millis,
                )),
            }))
            .await?;
        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.power.WaitForSuspendingToken".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(false)),
            }))
            .await?;

        let root_driver = match args.root_driver {
            Some(val) => val,
            None => "fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string(),
        };
        realm
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.driver.manager.RootDriver".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    root_driver.into(),
                )),
            }))
            .await?;

        // Setup software device config capabilities.
        let software_devs_src = match args.software_devices {
            Some(devs) => {
                let names = devs.iter().map(|dev| dev.device_name.clone()).collect::<Vec<_>>();
                let ids = devs.iter().map(|dev| dev.device_id).collect::<Vec<_>>();
                realm
                    .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                        name: "fuchsia.platform.bus.SoftwareDeviceNames".parse()?,
                        value: cm_rust::ConfigValue::Vector(
                            cm_rust::ConfigVectorValue::StringVector(
                                names.into_iter().map(FlyStr::new).collect::<Box<[_]>>(),
                            ),
                        ),
                    }))
                    .await?;
                realm
                    .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                        name: "fuchsia.platform.bus.SoftwareDeviceIds".parse()?,
                        value: cm_rust::ConfigValue::Vector(
                            cm_rust::ConfigVectorValue::Uint32Vector(ids.into()),
                        ),
                    }))
                    .await?;
                Ref::self_()
            }
            None => Ref::void(),
        };

        // Config routes.
        realm
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.driver.BindEager"))
                    .capability(Capability::configuration("fuchsia.driver.DisabledDrivers"))
                    .capability(Capability::configuration(
                        "fuchsia.driver.index.StopOnIdleTimeoutMillis",
                    ))
                    .from(Ref::self_())
                    .to(&driver_index),
            )
            .await?;

        realm
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.driver.manager.RootDriver"))
                    .from(Ref::self_())
                    .to(&driver_manager),
            )
            .await?;

        realm
            .add_route(
                Route::new()
                    .capability(
                        Capability::configuration(
                            "fuchsia.driver.manager.SetRootDriverHostCritical",
                        )
                        .optional(),
                    )
                    .capability(
                        Capability::configuration("fuchsia.driver.manager.SuspendTimeoutFallback")
                            .optional(),
                    )
                    .from(Ref::void())
                    .to(&driver_manager),
            )
            .await?;

        realm
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.driver.testrealm.TunnelBootItems",
                    ))
                    .capability(Capability::configuration("fuchsia.driver.testrealm.BoardName"))
                    .capability(Capability::configuration("fuchsia.driver.testrealm.PlatformVid"))
                    .capability(Capability::configuration("fuchsia.driver.testrealm.PlatformPid"))
                    .from(Ref::self_())
                    .to(&dtr_support),
            )
            .await?;

        realm
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.power.WaitForSuspendingToken"))
                    .from(Ref::self_())
                    .to(&driver_manager),
            )
            .await?;

        realm
            .add_route(
                Route::new()
                    .capability(
                        Capability::configuration("fuchsia.platform.bus.SoftwareDeviceNames")
                            .optional(),
                    )
                    .capability(
                        Capability::configuration("fuchsia.platform.bus.SoftwareDeviceIds")
                            .optional(),
                    )
                    .from(software_devs_src)
                    .to(&boot_drivers),
            )
            .await?;

        // Dynamic routes to the driver framework children.
        realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.driver.test.Internal"))
                    .from(&driver_test_internal)
                    .to(&fake_resolver),
            )
            .await?;

        // Routes from the driver framework children out to the test.
        self.add_route(
            Route::new()
                .capability(Capability::directory("dev-class"))
                .capability(Capability::directory("dev-topological"))
                .capability(Capability::protocol_by_name(
                    "fuchsia.driver.registrar.DriverRegistrar",
                ))
                .capability(Capability::protocol_by_name("fuchsia.driver.development.Manager"))
                .capability(Capability::protocol_by_name(
                    "fuchsia.driver.framework.CompositeNodeManager",
                ))
                .capability(Capability::protocol_by_name("fuchsia.driver.framework.NodeManager"))
                .capability(Capability::protocol_by_name("fuchsia.system.state.Administrator"))
                .from(&realm)
                .to(Ref::parent()),
        )
        .await?;
        // LINT.ThenChange(/sdk/lib/driver_test_realm/realm_builder/cpp/builder.cc)
        Ok(&self)
    }
}

#[async_trait::async_trait]
pub trait DriverTestRealmInstance {
    /// Connect to the /dev/ directory hosted by  DriverTestRealm in this Instance.
    fn driver_test_realm_connect_to_dev(&self) -> Result<fio::DirectoryProxy>;

    /// Waits for the driver manager boot up logic to complete. This will ensure all
    /// in-progress binds complete and indicates its safe to proceed with the test
    /// or tear down the test realm with no errors.
    async fn wait_for_bootup(&self) -> Result<()>;

    /// Waits for the node matching the given moniker.
    async fn wait_for_node(&self, moniker: &str) -> Result<fdd::NodeInfo>;
}

#[async_trait::async_trait]
impl DriverTestRealmInstance for RealmInstance {
    fn driver_test_realm_connect_to_dev(&self) -> Result<fio::DirectoryProxy> {
        fuchsia_fs::directory::open_directory_async(
            self.root.get_exposed_dir(),
            "dev-topological",
            fio::Flags::empty(),
        )
        .map_err(Into::into)
    }

    async fn wait_for_bootup(&self) -> Result<()> {
        let manager: fdd::ManagerProxy = self.root.connect_to_protocol_at_exposed_dir()?;
        manager.wait_for_bootup().await?;
        Ok(())
    }

    async fn wait_for_node(&self, moniker: &str) -> Result<fdd::NodeInfo> {
        let manager: fdd::ManagerProxy = self.root.connect_to_protocol_at_exposed_dir()?;
        loop {
            let (iterator, iterator_server) =
                fidl::endpoints::create_proxy::<fdd::NodeInfoIteratorMarker>();
            manager.get_node_info(&[moniker.to_string()], iterator_server, true)?;
            let next = iterator.get_next().await;
            if let Ok(nodes) = next
                && !nodes.is_empty()
                && nodes[0].moniker == Some(moniker.to_string())
            {
                return Ok(nodes[0].clone());
            }
        }
    }
}
