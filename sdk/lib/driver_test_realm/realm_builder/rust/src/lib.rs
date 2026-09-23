// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use cm_rust::push_box;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_io as fio;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};

pub const COMPONENT_NAME: &str = "driver_test_realm";
pub const DRIVER_TEST_REALM_URL: &str = "#meta/driver_test_realm.cm";

mod builder;
pub use builder::{
    DriverTestRealmBuilder as DriverTestRealmBuilder2,
    DriverTestRealmInstance as DriverTestRealmInstance2, Options as Options2,
};

/// Any additional options for the driver test realm setup.
pub struct Options {
    route_tracing_from_void: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { route_tracing_from_void: true }
    }
}

impl Options {
    /// Creates a new Options.
    pub fn new() -> Self {
        Self::default()
    }

    /// When set, the user should manually add a route for the "fuchsia.tracing.provider.Registry"
    /// capability.
    pub fn allow_external_tracing_route(mut self) -> Self {
        self.route_tracing_from_void = false;
        self
    }
}

#[async_trait::async_trait]
pub trait DriverTestRealmBuilder {
    /// Set up the DriverTestRealm component in the RealmBuilder realm.
    /// This configures proper input/output routing of capabilities.
    /// This takes a `manifest_url` to use, which is used by tests that need to
    /// specify a custom driver test realm.
    async fn driver_test_realm_manifest_setup(
        &self,
        manifest_url: &str,
        options: Options,
    ) -> Result<&Self>;
    /// Set up the DriverTestRealm component in the RealmBuilder realm.
    /// This configures proper input/output routing of capabilities.
    async fn driver_test_realm_setup(&self) -> Result<&Self>;

    /// For use in conjunction with `fuchsia.driver.test.RealmArgs/dtr_exposes` defined in
    /// `sdk/fidl/fuchsia.driver.test/realm.fidl`.
    /// Whenever a dtr_exposes is going to be provided to the RealmArgs, the user MUST call this
    /// function with a reference to the same dtr_exposes vector it intends to use. This will
    /// setup the necessary expose declarations inside the driver test realm and add the necessary
    /// realm_builder routes to support it.
    async fn driver_test_realm_add_dtr_exposes<'a>(
        &self,
        dtr_exposes: &'a [ftest::Capability],
    ) -> Result<&Self>;

    /// For use in conjunction with `fuchsia.driver.test.RealmArgs/dtr_offers` defined in
    /// `sdk/fidl/fuchsia.driver.test/realm.fidl`.
    /// Whenever a dtr_offers is going to be provided to the RealmArgs, the user MUST call this
    /// function with a reference to the same dtr_offers vector it intends to use. This will
    /// setup the necessary offers declarations inside the driver test realm and add the necessary
    /// realm_builder routes to support it.
    async fn driver_test_realm_add_dtr_offers<'a>(
        &self,
        dtr_offers: &'a [ftest::Capability],
        from: Ref,
    ) -> Result<&Self>;
}

#[async_trait::async_trait]
impl DriverTestRealmBuilder for RealmBuilder {
    async fn driver_test_realm_manifest_setup(
        &self,
        manifest_url: &str,
        options: Options,
    ) -> Result<&Self> {
        let driver_realm =
            self.add_child(COMPONENT_NAME, manifest_url, ChildOptions::new().eager()).await?;

        if options.route_tracing_from_void {
            self.add_route(
                Route::new()
                    .capability(
                        Capability::protocol_by_name("fuchsia.tracing.provider.Registry")
                            .optional(),
                    )
                    .from(Ref::void())
                    .to(&driver_realm),
            )
            .await?;
        }

        // Keep the rust and c++ realm_builders in sync with the driver_test_realm manifest.
        // LINT.IfChange
        // Uses from the driver_test_realm manifest.
        self.add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.inspect.InspectSink"))
                .capability(Capability::protocol_by_name("fuchsia.diagnostics.ArchiveAccessor"))
                .capability(
                    Capability::protocol_by_name("fuchsia.component.resolution.Resolver-hermetic")
                        .optional(),
                )
                .capability(
                    Capability::protocol_by_name("fuchsia.pkg.PackageResolver-hermetic").optional(),
                )
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::parent())
                .to(&driver_realm),
        )
        .await?;
        // Exposes from the the driver_test_realm manifest.
        self.add_route(
            Route::new()
                .capability(Capability::directory("dev-class").rights(fio::R_STAR_DIR))
                .capability(Capability::directory("dev-topological").rights(fio::R_STAR_DIR))
                .capability(Capability::protocol_by_name("fuchsia.system.state.Administrator"))
                .capability(Capability::protocol_by_name("fuchsia.driver.development.Manager"))
                .capability(Capability::protocol_by_name(
                    "fuchsia.driver.framework.CompositeNodeManager",
                ))
                .capability(Capability::protocol_by_name("fuchsia.driver.framework.NodeManager"))
                .capability(Capability::protocol_by_name(
                    "fuchsia.driver.registrar.DriverRegistrar",
                ))
                .capability(Capability::protocol_by_name("fuchsia.driver.test.Realm"))
                .from(&driver_realm)
                .to(Ref::parent()),
        )
        .await?;
        // LINT.ThenChange(/sdk/lib/driver_test_realm/realm_builder/cpp/builder.cc)
        Ok(&self)
    }

    async fn driver_test_realm_setup(&self) -> Result<&Self> {
        self.driver_test_realm_manifest_setup(DRIVER_TEST_REALM_URL, Options::default()).await
    }

    async fn driver_test_realm_add_dtr_exposes<'a>(
        &self,
        dtr_exposes: &'a [ftest::Capability],
    ) -> Result<&Self> {
        let mut decl = self.get_component_decl(COMPONENT_NAME).await?;
        for expose in dtr_exposes {
            let name = match expose {
                fidl_fuchsia_component_test::Capability::Protocol(p) => p.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Directory(d) => d.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Storage(s) => s.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Service(s) => s.name.as_ref(),
                fidl_fuchsia_component_test::Capability::EventStream(e) => e.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Config(c) => c.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Dictionary(d) => d.name.as_ref(),
                _ => None,
            };
            let expose_parsed = name
                .expect("No name found in capability.")
                .parse::<cm_types::Name>()
                .expect("Not a valid capability name");

            push_box(
                &mut decl.exposes,
                cm_rust::ExposeDecl::Service(cm_rust::ExposeServiceDecl {
                    source: cm_rust::ExposeSource::Collection(
                        "realm_builder".parse::<cm_types::Name>().unwrap(),
                    ),
                    source_name: expose_parsed.clone(),
                    source_dictionary: Default::default(),
                    target_name: expose_parsed.clone(),
                    target: cm_rust::ExposeTarget::Parent,
                    availability: cm_rust::Availability::Required,
                }),
            );
        }
        self.replace_component_decl(COMPONENT_NAME, decl).await?;

        for expose in dtr_exposes {
            // Add the route through the realm builder.
            self.add_route(
                Route::new()
                    .capability(expose.clone())
                    .from(Ref::child(COMPONENT_NAME))
                    .to(Ref::parent()),
            )
            .await?;
        }

        Ok(&self)
    }

    async fn driver_test_realm_add_dtr_offers<'a>(
        &self,
        dtr_offers: &'a [ftest::Capability],
        from: Ref,
    ) -> Result<&Self> {
        let mut decl = self.get_component_decl(COMPONENT_NAME).await?;
        for offer in dtr_offers {
            let name = match offer {
                fidl_fuchsia_component_test::Capability::Protocol(p) => p.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Directory(d) => d.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Storage(s) => s.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Service(s) => s.name.as_ref(),
                fidl_fuchsia_component_test::Capability::EventStream(e) => e.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Config(c) => c.name.as_ref(),
                fidl_fuchsia_component_test::Capability::Dictionary(d) => d.name.as_ref(),
                _ => None,
            };
            let offer_parsed = name
                .expect("No name found in capability.")
                .parse::<cm_types::Name>()
                .expect("Not a valid capability name");

            push_box(
                &mut decl.offers,
                cm_rust::offer::OfferDecl::Protocol(cm_rust::offer::OfferProtocolDecl {
                    source: cm_rust::offer::OfferSource::Parent,
                    source_name: offer_parsed.clone(),
                    source_dictionary: Default::default(),
                    target_name: offer_parsed.clone(),
                    target: cm_rust::offer::OfferTarget::Collection(
                        "realm_builder".parse::<cm_types::Name>().unwrap(),
                    ),
                    dependency_type: cm_rust::DependencyType::Strong,
                    availability: cm_rust::Availability::Required,
                }),
            );
        }
        self.replace_component_decl(COMPONENT_NAME, decl).await?;

        for offer in dtr_offers {
            // Add the route through the realm builder.
            self.add_route(
                Route::new()
                    .capability(offer.clone())
                    .from(from.clone())
                    .to(Ref::child(COMPONENT_NAME)),
            )
            .await?;
        }

        Ok(&self)
    }
}

#[async_trait::async_trait]
pub trait DriverTestRealmInstance {
    /// Connect to the DriverTestRealm in this Instance and call Start with `args`.
    async fn driver_test_realm_start(&self, args: fdt::RealmArgs) -> Result<()>;

    /// Connect to the /dev/ directory hosted by  DriverTestRealm in this Instance.
    fn driver_test_realm_connect_to_dev(&self) -> Result<fio::DirectoryProxy>;
}

#[async_trait::async_trait]
impl DriverTestRealmInstance for RealmInstance {
    async fn driver_test_realm_start(&self, args: fdt::RealmArgs) -> Result<()> {
        let config: fdt::RealmProxy = self.root.connect_to_protocol_at_exposed_dir()?;
        let () = config
            .start(args)
            .await
            .context("DriverTestRealm Start failed")?
            .map_err(zx::Status::err_from_raw)
            .context("DriverTestRealm Start failed")?;
        Ok(())
    }

    fn driver_test_realm_connect_to_dev(&self) -> Result<fio::DirectoryProxy> {
        fuchsia_fs::directory::open_directory_async(
            self.root.get_exposed_dir(),
            "dev-topological",
            fio::Flags::empty(),
        )
        .map_err(Into::into)
    }
}
