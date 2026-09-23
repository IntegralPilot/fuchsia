// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use crate::util;
use anyhow::{Context as _, anyhow, bail};
use assembly_config_schema::BoardConfig;
use assembly_config_schema::assembly_input_bundle::{
    CompiledComponentDefinition, CompiledPackageDefinition,
};
use assembly_config_schema::platform_settings::recovery_config::RecoveryConfig;
use assembly_config_schema::platform_settings::session_config::PlatformSessionConfig;
use assembly_config_schema::product_settings::{GlobalPlatformTee, GlobalPlatformTeeClient, Tee};
use assembly_constants::{BlobfsCompiledPackageDestination, CompiledPackageDestination, FileEntry};
use cml::types::capability::Capability;
use cml::types::child::Child;
use cml::types::document::Document;
use cml::types::expose::{Expose, ExposeFromRef};
use cml::types::offer::{Offer, OfferFromRef, OfferToRef};
use cml::types::right::{Right, Rights};
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use std::io::Write as _;

pub(crate) struct TeeConfig;
impl
    DefineSubsystemConfiguration<(
        &Tee,
        &Vec<GlobalPlatformTeeClient>,
        &RecoveryConfig,
        &PlatformSessionConfig,
    )> for TeeConfig
{
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        product_config: &(
            &Tee,
            &Vec<GlobalPlatformTeeClient>,
            &RecoveryConfig,
            &PlatformSessionConfig,
        ),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        let global_platform_trusted_app_guids =
            get_global_platform_tee_trusted_app_guids(context.board_config)?;
        let (tee, transitional_tee_clients, recovery_config, session) = *product_config;
        if tee != &Tee::Undefined && !transitional_tee_clients.is_empty() {
            bail!(
                "Conflicting TEE configuration: product configuration may contain `tee` or `tee_clients`, but not both"
            );
        }

        if tee == &Tee::NoTee {
            return Ok(());
        }

        // The proprietary TEE realm URL is provided by the board config for Standard
        // feature set level builds.
        let board_tee_url = if *context.feature_set_level == FeatureSetLevel::Standard {
            context.board_config.platform.security.tee_realm_url.as_deref()
        } else {
            None
        };

        if let Some(url) = board_tee_url {
            define_proprietary_tee_configuration(context, url, recovery_config, session, builder)
        } else {
            match tee {
                Tee::NoTee => unreachable!(),
                Tee::Undefined => define_global_platform_tee_configuration(
                    context,
                    transitional_tee_clients,
                    recovery_config,
                    global_platform_trusted_app_guids,
                    session.enabled,
                    builder,
                ),
                Tee::GlobalPlatform(GlobalPlatformTee { clients }) => {
                    define_global_platform_tee_configuration(
                        context,
                        clients,
                        recovery_config,
                        global_platform_trusted_app_guids,
                        session.enabled,
                        builder,
                    )
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
struct TeeManagerConfig {
    application_uuids: Vec<uuid::Uuid>,
}

fn create_name(name: &str) -> Result<cml::Name, anyhow::Error> {
    cml::Name::new(name).with_context(|| format!("Invalid name: {name}"))
}

fn define_global_platform_tee_configuration(
    context: &ConfigurationContext<'_>,
    product_config: &Vec<GlobalPlatformTeeClient>,
    recovery_config: &RecoveryConfig,
    tee_trusted_app_guids: &Vec<uuid::Uuid>,
    session_enabled: bool,
    builder: &mut dyn ConfigurationBuilder,
) -> anyhow::Result<()> {
    match context.feature_set_level {
        // tee_manager and clients only exist in systems that have the core realm
        FeatureSetLevel::Embeddable | FeatureSetLevel::Bootstrap => return Ok(()),
        FeatureSetLevel::Utility | FeatureSetLevel::Standard => {}
    }

    // Configure the tee_manager based on whether the board provided GUIDs
    // to serve from it.
    if !tee_trusted_app_guids.is_empty() {
        create_tee_manager(tee_trusted_app_guids, context, builder)?;
        if recovery_config.has_factory_reset(*context.feature_set_level) {
            builder.platform_bundle("factory_reset_tee")?;
        }
    } else {
        builder.core_shard(&context.get_resource("no_tee_manager.core_shard.cml"));
        if recovery_config.has_factory_reset(*context.feature_set_level) {
            builder.platform_bundle("factory_reset_no_tee")?;
        }
        if session_enabled {
            builder.core_shard(&context.get_resource("no_tee_manager.session.core_shard.cml"));
        }
    }

    // Hook up the clients of the tee_manager that the product has
    // registered to run under the session.
    if !product_config.is_empty() {
        create_tee_clients(product_config, context, builder)?;
    }

    Ok(())
}

fn create_tee_manager(
    tee_trusted_app_guids: &Vec<uuid::Uuid>,
    context: &ConfigurationContext<'_>,
    builder: &mut dyn ConfigurationBuilder,
) -> Result<(), anyhow::Error> {
    let tee_manager_config =
        TeeManagerConfig { application_uuids: (*tee_trusted_app_guids).clone() };
    let gendir = context.get_gendir()?;

    // Pull in the tee_manager platform bundle, which gives us the tee_manager
    // binary and various CML includes, as well as the tee_manager core shard.
    builder.platform_bundle("tee_manager")?;

    // Write tee_manager's config-data
    let tee_manager_config_path = gendir.join("tee_manager.config");
    let mut tee_manager_config_file = std::fs::File::create(&tee_manager_config_path)?;
    tee_manager_config_file
        .write_all(serde_json::to_string_pretty(&tee_manager_config)?.as_bytes())?;
    builder.package("tee_manager").config_data(FileEntry {
        source: tee_manager_config_path,
        destination: "tee_manager.config".into(),
    })?;

    // Create the tee_manager component definition itself.
    // tee_manager declares a capability and an expose for each GUID it is given
    // in the board configuration, as a protocol with a specific name format.
    let protocols: Vec<Option<cml::OneOrMany<cml::Name>>> = tee_trusted_app_guids
        .iter()
        .map(|guid| create_name(&format!("fuchsia.tee.Application.{guid}")))
        .collect::<Result<Vec<cml::Name>, anyhow::Error>>()?
        .iter()
        .map(|name| Some(cml::OneOrMany::One(name.clone())))
        .collect();

    let capabilities = protocols
        .iter()
        .map(|protocol| Capability { protocol: protocol.clone(), ..Default::default() })
        .collect();

    let expose = protocols
        .iter()
        .map(|protocol| Expose {
            protocol: protocol.clone(),
            ..Expose::new_from(ExposeFromRef::Self_.into())
        })
        .collect();

    // Serialize the component.
    let cml = Document {
        capabilities: Some(capabilities),
        expose: Some(expose),
        include: Some(vec!["tee_manager.base.cml".into()]),
        ..Default::default()
    };
    let cml_name = "tee_manager.cml";
    let cml_path = gendir.join(cml_name);
    let mut cml_file = std::fs::File::create(&cml_path)?;
    cml_file.write_all(serde_json::to_string_pretty(&cml)?.as_bytes())?;
    let components = vec![CompiledComponentDefinition {
        component_name: "tee_manager".into(),
        shards: vec![cml_path],
        cmc_features: vec![],
    }];
    let destination =
        CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::TeeManager);
    let def = CompiledPackageDefinition {
        name: destination.clone(),
        components,

        // Contents and includes are set by the tee_manager AIB defined in
        // //bundles/assembly/BUILD.gn. This prevents us having to make
        // all of the contents and includes into assembly resources.
        contents: vec![],
        includes: vec![],
        bootfs_package: false,
    };
    builder
        .compiled_package(destination.clone(), def)
        .with_context(|| format!("Inserting compiled package: {destination}"))?;

    Ok(())
}

fn create_tee_clients(
    product_config: &Vec<GlobalPlatformTeeClient>,
    context: &ConfigurationContext<'_>,
    builder: &mut dyn ConfigurationBuilder,
) -> Result<(), anyhow::Error> {
    let gendir = context.get_gendir()?;

    let capabilities = vec![Capability {
        dictionary: Some(create_name("tee-client-capabilities")?),
        ..Default::default()
    }];
    let expose = vec![Expose {
        dictionary: Some(create_name("tee-client-capabilities")?.into()),
        ..Expose::new_from(ExposeFromRef::Self_.into())
    }];

    let mut children = vec![];

    let mut offer = vec![
        Offer {
            dictionary: Some(create_name("diagnostics")?.into()),
            ..Offer::empty(OfferFromRef::Parent.into(), OfferToRef::All.into())
        },
        Offer {
            protocol: Some(create_name("fuchsia.tracing.provider.Registry")?.into()),
            availability: Some(cml::Availability::SameAsTarget),
            ..Offer::empty(OfferFromRef::Parent.into(), OfferToRef::All.into())
        },
    ];

    for tee_client in product_config {
        let component_url = AbsoluteComponentUrl::parse(&tee_client.component_url)?;
        let component_name = create_name(
            component_url
                .resource()
                .split('/')
                .next_back()
                .ok_or_else(|| anyhow!("no resource name: {}", component_url.resource()))?
                .split('.')
                .next()
                .ok_or_else(|| anyhow!("no component name: {}", component_url.resource()))?,
        )?;
        children.push(Child {
            name: component_name.clone(),
            url: cm_types::Url::new(component_url.to_string())?,
            startup: cml::StartupMode::Lazy,
            on_terminate: None,
            environment: None,
        });

        for capability in &tee_client.capabilities {
            // Expose the capabilities up from the component URL to the
            // dictionary we provide to the parent
            offer.push(Offer {
                protocol: Some(create_name(capability)?.into()),
                availability: Some(cml::Availability::SameAsTarget),
                ..Offer::empty(
                    OfferFromRef::Named(component_name.clone()).into(),
                    OfferToRef::OwnDictionary(create_name("tee-client-capabilities")?).into(),
                )
            });
        }

        for guid in &tee_client.guids {
            // Expose the guids from tee_manager to the component in question
            let guid_protocol_name = create_name(&format!("fuchsia.tee.Application.{guid}"))?;
            offer.push(Offer {
                protocol: Some(guid_protocol_name.into()),
                ..Offer::empty(
                    OfferFromRef::Parent.into(),
                    OfferToRef::Named(component_name.clone()).into(),
                )
            });
        }

        for protocol in &tee_client.additional_required_protocols {
            let protocol_name = create_name(protocol)?;
            offer.push(Offer {
                protocol: Some(protocol_name.into()),

                // Most of these additional capabilities will come from
                // tee_manager or factory_store_providers, and not all
                // boards contain those components.
                source_availability: Some(cml::SourceAvailability::Unknown),
                ..Offer::empty(
                    OfferFromRef::Parent.into(),
                    OfferToRef::Named(component_name.clone()).into(),
                )
            });
        }

        if let Some(config_data) = &tee_client.config_data {
            // For each key in config data, add the file at the path of the value to config data
            let package_name = component_url.package_url().name();

            for (key, value) in &config_data.files {
                builder
                    .package(package_name.as_ref())
                    .config_data(FileEntry { source: value.clone(), destination: key.into() })
                    .context(format!("Adding config data file {key} to package {package_name}"))?;
            }

            // Route the config-data subdir named with the package-name to this component
            let directory_name = create_name("config-data")?;
            let subdir_name: cml::RelativePath = cm_types::RelativePath::new(package_name)?;
            offer.push(Offer {
                directory: Some(directory_name.into()),
                subdir: Some(subdir_name),
                ..Offer::empty(
                    OfferFromRef::Parent.into(),
                    OfferToRef::Named(component_name.clone()).into(),
                )
            })
        }

        if tee_client.additional_required_features.persistent_storage {
            offer.push(Offer {
                storage: Some(create_name("data")?.into()),
                ..Offer::empty(
                    OfferFromRef::Parent.into(),
                    OfferToRef::Named(component_name.clone()).into(),
                )
            })
        }

        if tee_client.additional_required_features.tmp_storage {
            offer.push(Offer {
                storage: Some(create_name("tmp")?.into()),
                ..Offer::empty(
                    OfferFromRef::Parent.into(),
                    OfferToRef::Named(component_name.clone()).into(),
                )
            })
        }

        if tee_client.additional_required_features.securemem {
            offer.push(Offer {
                directory: Some(create_name("dev-securemem")?.into()),
                rights: Some(Rights(vec![Right::ReadAlias])),
                ..Offer::empty(
                    OfferFromRef::Parent.into(),
                    OfferToRef::Named(component_name).into(),
                )
            })
        }
    }

    let cml = Document {
        capabilities: Some(capabilities),
        expose: Some(expose),
        children: Some(children),
        offer: Some(offer),
        ..Default::default()
    };

    let cml_name = "tee-clients.cml";
    let cml_path = gendir.join(cml_name);
    let mut cml_file = std::fs::File::create(&cml_path)?;

    cml_file.write_all(serde_json::to_string_pretty(&cml)?.as_bytes())?;
    let components = vec![CompiledComponentDefinition {
        component_name: "tee-clients".into(),
        shards: vec![cml_path],
        cmc_features: vec![],
    }];

    let destination =
        CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::TeeClients);
    let def = CompiledPackageDefinition {
        name: destination.clone(),
        components,
        contents: Default::default(),
        includes: Default::default(),
        bootfs_package: false,
    };

    builder
        .compiled_package(destination.clone(), def)
        .with_context(|| format!("Inserting compiled package: {destination}"))?;
    builder.core_shard(&context.get_resource("tee-clients.core_shard.cml"));

    Ok(())
}

fn get_global_platform_tee_trusted_app_guids(
    board_config: &BoardConfig,
) -> anyhow::Result<&Vec<uuid::Uuid>> {
    if !board_config.global_platform_tee_trusted_app_guids.is_empty()
        && !board_config.tee_trusted_app_guids.is_empty()
    {
        bail!(
            "Cannot set both `global_platform_tee_trusted_app_guids` and deprecated `tee_trusted_app_guids`"
        );
    }

    Ok(if board_config.tee_trusted_app_guids.is_empty() {
        &board_config.global_platform_tee_trusted_app_guids
    } else {
        &board_config.tee_trusted_app_guids
    })
}

fn define_proprietary_tee_configuration(
    context: &ConfigurationContext<'_>,
    tee_realm_url: &str,
    recovery_config: &RecoveryConfig,
    session: &PlatformSessionConfig,
    builder: &mut dyn ConfigurationBuilder,
) -> anyhow::Result<()> {
    let core_shard_template = if session.enabled {
        "proprietary_tee_manager.session.core_shard.cml.template"
    } else {
        "proprietary_tee_manager.no_session.core_shard.cml.template"
    };
    util::add_platform_declared_product_provided_component(
        tee_realm_url,
        core_shard_template,
        context,
        builder,
    )?;
    if recovery_config.has_factory_reset(*context.feature_set_level) {
        builder.platform_bundle("factory_reset_no_tee")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompletedConfiguration;
    use crate::subsystems::ConfigurationBuilderImpl;
    use assembly_config_schema::BoardConfig;
    use assembly_config_schema::product_settings::{TeeClientConfigData, TeeClientFeatures};
    use assembly_images_config::BoardFilesystemConfig;
    use camino::{Utf8Path, Utf8PathBuf};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::LazyLock;

    static BOARD_INFO_WITH_GLOBAL_PLATFORM_TEE_TRUSTED_APP_GUIDS: LazyLock<BoardConfig> =
        LazyLock::new(|| BoardConfig {
            name: "Test Board".into(),
            provided_features: vec![],
            input_bundles: Default::default(),
            filesystems: BoardFilesystemConfig::default(),
            global_platform_tee_trusted_app_guids: vec![
                uuid::Uuid::parse_str("9105f952-86db-4808-bc0e-8a4172e11843").unwrap(),
            ],
            ..Default::default()
        });

    static BOARD_INFO_WITH_TEE_TRUSTED_APP_GUIDS: LazyLock<BoardConfig> =
        LazyLock::new(|| BoardConfig {
            name: "Test Board".into(),
            provided_features: vec![],
            input_bundles: Default::default(),
            filesystems: BoardFilesystemConfig::default(),
            tee_trusted_app_guids: vec![
                uuid::Uuid::parse_str("9105f952-86db-4808-bc0e-8a4172e11843").unwrap(),
            ],
            ..Default::default()
        });

    static BOARD_INFO_WITH_GLOBAL_PLATFORM_TEE_TRUSTED_APP_GUIDS_AND_TEE_TRUSTED_APP_GUIDS:
        LazyLock<BoardConfig> = LazyLock::new(|| BoardConfig {
        name: "Test Board".into(),
        provided_features: vec![],
        input_bundles: Default::default(),
        filesystems: BoardFilesystemConfig::default(),
        global_platform_tee_trusted_app_guids: vec![
            uuid::Uuid::parse_str("9105f952-86db-4808-bc0e-8a4172e11843").unwrap(),
        ],
        tee_trusted_app_guids: vec![
            uuid::Uuid::parse_str("9105f952-86db-4808-bc0e-8a4172e11843").unwrap(),
        ],
        ..Default::default()
    });

    #[test]
    // This test is a change detector, but we actually want to observe changes
    // in this type of code, which is dynamically generating CML for
    // components which might have security implications.
    fn test_tee_clients() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        let (context, tee_client_config, tee_trusted_app_guids, mut builder) =
            setup_test(resource_dir.path());

        define_global_platform_tee_configuration(
            &context,
            &tee_client_config,
            &RecoveryConfig::default(),
            &tee_trusted_app_guids,
            true,
            &mut builder,
        )
        .expect("defining tee_clients configuration");

        let completed_configuration = builder.build();
        let compiled_packages = completed_configuration.compiled_packages;
        assert_eq!(compiled_packages.len(), 2);

        let compiled_package = compiled_packages
            .get(&CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::TeeClients))
            .unwrap();

        // Verify that we created tee-clients correctly
        let shard: Utf8PathBuf;
        if let CompiledPackageDefinition {
            name: CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::TeeClients),
            components,
            contents,
            includes,
            bootfs_package: false,
        } = compiled_package
        {
            assert_eq!(components.len(), 1);
            let component = &components[0];
            assert_eq!(contents.len(), 0);
            assert_eq!(includes.len(), 0);

            assert_eq!(component.component_name, "tee-clients");
            assert_eq!(component.shards.len(), 1);

            shard = component.shards[0].clone();
        } else {
            panic!("unexpected compiled package definition: {compiled_package:#?}");
        }

        let contents = std::fs::read_to_string(shard.clone()).unwrap();
        eprintln!("contents: {contents}");
        eprintln!("path: {shard}");
        let contents_json: serde_json::Value =
            serde_json::from_str(&contents).expect("parsing cml");

        let expected_json = serde_json::json!({"children": [
          {
            "name": "test-app",
            "url": "fuchsia-pkg://fuchsia.com/tee-clients/test-app#meta/test-app.cm"
          }
        ],
        "capabilities": [
          {
            "dictionary": "tee-client-capabilities"
          }
        ],
        "expose": [
          {
            "dictionary": "tee-client-capabilities",
            "from": "self"
          }
        ],
        "offer": [
          {
            "dictionary": "diagnostics",
            "from": "parent",
            "to": "all"
          },
          {
            "protocol": "fuchsia.tracing.provider.Registry",
            "from": "parent",
            "to": "all",
            "availability": "same_as_target",
          },
          {
            "protocol": "fuchsia.baz.bang",
            "from": "#test-app",
            "to": "self/tee-client-capabilities",
            "availability": "same_as_target"
          },
          {
            "protocol": "fuchsia.tee.Application.1234",
            "from": "parent",
            "to": "#test-app"
          },
          {
            "protocol": "fuchsia.tee.Application.5678",
            "from": "parent",
            "to": "#test-app"
          },
          {
            "protocol": "fuchsia.foo.bar",
            "from": "parent",
            "to": "#test-app",
            "source_availability": "unknown"
          },
          {
            "directory": "config-data",
            "from": "parent",
            "to": "#test-app",
            "subdir": "tee-clients",
          },
          {
            "storage": "data",
            "from": "parent",
            "to": "#test-app",
          },
          {
            "storage": "tmp",
            "from": "parent",
            "to": "#test-app",
          },
          {
            "directory": "dev-securemem",
            "from": "parent",
            "rights": ["r*"],
            "to": "#test-app",
          }
        ]});

        assert_eq!(
            expected_json, contents_json,
            "cml mismatch: Expected: \n\n{expected_json:#?}\n\nActual:\n\n{contents_json:#?}"
        );
    }

    #[test]
    fn test_tee_manager() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        let (context, tee_client_config, tee_trusted_app_guids, mut builder) =
            setup_test(resource_dir.path());
        define_global_platform_tee_configuration(
            &context,
            &tee_client_config,
            &RecoveryConfig::default(),
            &tee_trusted_app_guids,
            true,
            &mut builder,
        )
        .expect("defining tee_clients configuration");

        let completed_configuration = builder.build();
        let compiled_packages = completed_configuration.compiled_packages;
        assert_eq!(compiled_packages.len(), 2);

        // Verify that we hooked up tee_manager correctly, including
        // - config data
        // - core shard
        // - routing from tee_manager to other components
        let tee_manager_config_data = &completed_configuration
            .package_configs
            .get("tee_manager")
            .expect("getting config data for tee_manager")
            .config_data;
        let tee_manager_config_path = &tee_manager_config_data
            .map
            .entries
            .get(&String::from("tee_manager.config"))
            .expect("getting tee manager config")
            .source;
        let tee_manager_config =
            std::fs::read_to_string(tee_manager_config_path).expect("opening tee manager config");
        let tee_manager_config_json: serde_json::Value =
            serde_json::from_str(&tee_manager_config).expect("parsing config");

        let expected_json = serde_json::json!({"application_uuids": [
            "9105f952-86db-4808-bc0e-8a4172e11843",
            "826a0526-cd9b-4a61-a96e-1fd5b53061a3",
            ]
        });

        assert_eq!(
            expected_json, tee_manager_config_json,
            "config mismatch: Expected: \n\n{expected_json:#?}\n\nActual:\n\n{tee_manager_config_json:#?}"
        );

        assert!(completed_configuration.bundles.contains("tee_manager"));

        let compiled_package = compiled_packages
            .get(&CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::TeeManager))
            .unwrap();

        // Verify that we created tee_manager correctly
        let shard: Utf8PathBuf;
        if let CompiledPackageDefinition {
            name: CompiledPackageDestination::Blob(BlobfsCompiledPackageDestination::TeeManager),
            components,
            contents,
            includes,
            bootfs_package: false,
        } = compiled_package
        {
            assert_eq!(components.len(), 1);
            let component = &components[0];
            assert_eq!(contents.len(), 0); // contents come from the tee_manager AIB.
            assert_eq!(includes.len(), 0);

            assert_eq!(component.component_name, "tee_manager");
            assert_eq!(component.shards.len(), 1);

            shard = component.shards[0].clone();
        } else {
            panic!("unexpected compiled package definition: {compiled_package:#?}");
        }

        let contents = std::fs::read_to_string(shard).unwrap();
        let contents_json: serde_json::Value =
            serde_json::from_str(&contents).expect("parsing cml");

        let expected_json = serde_json::json!({
            "include": ["tee_manager.base.cml"],
            "capabilities": [
                {
                    "protocol": "fuchsia.tee.Application.9105f952-86db-4808-bc0e-8a4172e11843",
                },
                {
                    "protocol" : "fuchsia.tee.Application.826a0526-cd9b-4a61-a96e-1fd5b53061a3",
                }
            ],
            "expose": [
                {
                    "from": "self",
                    "protocol" :"fuchsia.tee.Application.9105f952-86db-4808-bc0e-8a4172e11843",
                },
                {
                    "from": "self",
                    "protocol" : "fuchsia.tee.Application.826a0526-cd9b-4a61-a96e-1fd5b53061a3",
                }
            ],
        });

        assert_eq!(
            expected_json, contents_json,
            "cml mismatch: Expected: \n\n{expected_json:#?}\n\nActual:\n\n{contents_json:#?}"
        );
    }

    fn setup_test(
        resource_dir: &Path,
    ) -> (
        ConfigurationContext<'static>,
        Vec<GlobalPlatformTeeClient>,
        Vec<uuid::Uuid>,
        ConfigurationBuilderImpl,
    ) {
        populate_resource_dir(resource_dir);

        let mut context = ConfigurationContext::default_for_tests();
        context.resource_dir = Utf8Path::from_path(resource_dir).unwrap().to_path_buf();
        let context = context;

        let tee_client_config = vec![GlobalPlatformTeeClient {
            component_url: "fuchsia-pkg://fuchsia.com/tee-clients/test-app#meta/test-app.cm"
                .to_string(),
            guids: vec!["1234".to_string(), "5678".to_string()],
            additional_required_protocols: vec!["fuchsia.foo.bar".to_string()],
            capabilities: vec!["fuchsia.baz.bang".to_string()],
            config_data: Some(TeeClientConfigData {
                files: BTreeMap::from([
                    ("foo".to_string(), "bar".into()),
                    ("baz".to_string(), "qux".into()),
                ]),
            }),
            additional_required_features: TeeClientFeatures {
                tmp_storage: true,
                persistent_storage: true,
                securemem: true,
            },
        }];

        let tee_trusted_app_guids = vec![
            uuid::Uuid::parse_str("9105f952-86db-4808-bc0e-8a4172e11843").unwrap(),
            uuid::Uuid::parse_str("826a0526-cd9b-4a61-a96e-1fd5b53061a3").unwrap(),
        ];

        let builder = ConfigurationBuilderImpl::default();
        (context, tee_client_config, tee_trusted_app_guids, builder)
    }

    #[test]
    fn valid_transitional_config() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        let tee_clients = non_empty_tee_clients();
        for context in [
            context_with_global_platform_trusted_app_guids(resource_dir.path()),
            context_with_tee_trusted_app_guids(resource_dir.path()),
        ] {
            let mut builder = ConfigurationBuilderImpl::default();
            TeeConfig::define_configuration(
                &context,
                &(&Default::default(), &tee_clients, &Default::default(), &Default::default()),
                &mut builder,
            )
            .expect("default (undefined) `tee` and `tee_clients` together is valid");
        }
    }

    #[test]
    fn invalid_transitional_config() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        let tee_clients = non_empty_tee_clients();
        for tee_config in [Tee::NoTee, Tee::GlobalPlatform(Default::default())] {
            for context in [
                context_with_global_platform_trusted_app_guids(resource_dir.path()),
                context_with_tee_trusted_app_guids(resource_dir.path()),
            ] {
                let mut builder = ConfigurationBuilderImpl::default();
                TeeConfig::define_configuration(
                    &context,
                    &(&tee_config, &tee_clients, &Default::default(), &Default::default()),
                    &mut builder,
                )
                .expect_err("`tee` and `tee_clients` together is invalid");
            }
        }

        let mut builder = ConfigurationBuilderImpl::default();
        TeeConfig::define_configuration(
            &context_with_global_platform_trusted_app_guids_and_tee_trusted_app_guids(
                resource_dir.path(),
            ),
            &(&Default::default(), &Default::default(), &Default::default(), &Default::default()),
            &mut builder,
        )
        .expect_err(
            "`global_platform_trusted_app_guids` and `tee_trusted_app_guids` together is invalid",
        );
    }

    #[test]
    fn valid_strategy_config() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        for tee_config in [Tee::NoTee, Tee::GlobalPlatform(Default::default())] {
            for context in [
                context_with_global_platform_trusted_app_guids(resource_dir.path()),
                context_with_tee_trusted_app_guids(resource_dir.path()),
            ] {
                let mut builder = ConfigurationBuilderImpl::default();
                TeeConfig::define_configuration(
                    &context,
                    &(&tee_config, &Default::default(), &Default::default(), &Default::default()),
                    &mut builder,
                )
                .expect("defined `tee` and empty `tee_clients` together is valid");
            }
        }
    }

    #[test]
    fn proprietary_session_dependency() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        let tee_config = Tee::Undefined;
        for board_config in [
            &*BOARD_INFO_WITH_GLOBAL_PLATFORM_TEE_TRUSTED_APP_GUIDS,
            &*BOARD_INFO_WITH_TEE_TRUSTED_APP_GUIDS,
        ] {
            let mut board_config = board_config.clone();
            board_config.platform.security.tee_realm_url =
                Some(String::from("fuchsia-pkg://test.fuchsia.com/my_tee#my_realm.cm"));
            populate_resource_dir(resource_dir.path());
            let mut context = ConfigurationContext::default_for_tests();
            context.board_config = &board_config;
            context.resource_dir = Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf();
            let build = |session| {
                let mut builder = ConfigurationBuilderImpl::default();
                TeeConfig::define_configuration(
                    &context,
                    &(&tee_config, &Default::default(), &Default::default(), &session),
                    &mut builder,
                )
                .expect("defined `tee` and empty `tee_clients` together is valid");
                builder.build()
            };
            let contains_core_shard = |config: &CompletedConfiguration, path_fragment| {
                config.core_shards.iter().any(|path| path.as_str().contains(path_fragment))
            };

            let session_config =
                build(PlatformSessionConfig { enabled: true, ..Default::default() });
            assert!(contains_core_shard(
                &session_config,
                "proprietary_tee_manager.session.core_shard.cml.template"
            ));
            assert!(!contains_core_shard(
                &session_config,
                "proprietary_tee_manager.no_session.core_shard.cml.template"
            ));

            let no_session_config =
                build(PlatformSessionConfig { enabled: false, ..Default::default() });
            assert!(contains_core_shard(
                &no_session_config,
                "proprietary_tee_manager.no_session.core_shard.cml.template"
            ));
            assert!(!contains_core_shard(
                &no_session_config,
                "proprietary_tee_manager.session.core_shard.cml.template"
            ));
        }
    }

    fn non_empty_tee_clients() -> Vec<GlobalPlatformTeeClient> {
        vec![GlobalPlatformTeeClient {
            component_url: "fuchsia-pkg://test.fuchsia.com/tee-clients/test-app#meta/test-app.cm"
                .to_string(),
            guids: vec!["1234".to_string(), "5678".to_string()],
            additional_required_protocols: vec!["fuchsia.foo.bar".to_string()],
            capabilities: vec!["fuchsia.baz.bang".to_string()],
            config_data: Some(TeeClientConfigData {
                files: BTreeMap::from([
                    ("foo".to_string(), "bar".into()),
                    ("baz".to_string(), "qux".into()),
                ]),
            }),
            additional_required_features: TeeClientFeatures {
                tmp_storage: true,
                persistent_storage: true,
                securemem: true,
            },
        }]
    }

    fn context_with_global_platform_trusted_app_guids(
        resource_dir: &Path,
    ) -> ConfigurationContext<'static> {
        populate_resource_dir(resource_dir);

        let mut context = ConfigurationContext::default_for_tests();
        context.board_config = &*BOARD_INFO_WITH_GLOBAL_PLATFORM_TEE_TRUSTED_APP_GUIDS;
        context.resource_dir = Utf8Path::from_path(resource_dir).unwrap().to_path_buf();
        context
    }

    fn context_with_tee_trusted_app_guids(resource_dir: &Path) -> ConfigurationContext<'static> {
        populate_resource_dir(resource_dir);

        let mut context = ConfigurationContext::default_for_tests();
        context.board_config = &*BOARD_INFO_WITH_TEE_TRUSTED_APP_GUIDS;
        context.resource_dir = Utf8Path::from_path(resource_dir).unwrap().to_path_buf();
        context
    }

    fn context_with_global_platform_trusted_app_guids_and_tee_trusted_app_guids(
        resource_dir: &Path,
    ) -> ConfigurationContext<'static> {
        populate_resource_dir(resource_dir);

        let mut context = ConfigurationContext::default_for_tests();
        context.board_config =
            &*BOARD_INFO_WITH_GLOBAL_PLATFORM_TEE_TRUSTED_APP_GUIDS_AND_TEE_TRUSTED_APP_GUIDS;
        context.resource_dir = Utf8Path::from_path(resource_dir).unwrap().to_path_buf();
        context
    }

    #[test]
    fn board_provided_tee_realm_url() {
        let resource_dir = tempfile::TempDir::new().unwrap();
        populate_resource_dir(resource_dir.path());

        let mut board_config = BoardConfig { name: "Test Board".into(), ..Default::default() };
        board_config.platform.security.tee_realm_url =
            Some("fuchsia-pkg://test.fuchsia.com/board_tee#meta/realm.cm".into());

        let mut context = ConfigurationContext::default_for_tests();
        context.board_config = &board_config;
        context.resource_dir = Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf();

        // 1. Board config URL with Tee::Undefined in product config.
        let mut builder = ConfigurationBuilderImpl::default();
        TeeConfig::define_configuration(
            &context,
            &(&Tee::Undefined, &Default::default(), &Default::default(), &Default::default()),
            &mut builder,
        )
        .expect("defining configuration with board tee_realm_url");
        let completed = builder.build();
        assert!(
            completed
                .core_shards
                .iter()
                .any(|path| path.as_str().contains("proprietary_tee_manager"))
        );

        // 2. Board config URL is ignored on non-Standard feature set levels
        // (bringup, zedboot/recovery).
        for level in
            [FeatureSetLevel::Bootstrap, FeatureSetLevel::Utility, FeatureSetLevel::Embeddable]
        {
            let mut context_non_standard = ConfigurationContext::default_for_tests();
            context_non_standard.board_config = &board_config;
            context_non_standard.feature_set_level = &level;
            context_non_standard.resource_dir =
                Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf();
            let mut builder = ConfigurationBuilderImpl::default();
            TeeConfig::define_configuration(
                &context_non_standard,
                &(&Tee::Undefined, &Default::default(), &Default::default(), &Default::default()),
                &mut builder,
            )
            .expect("defining configuration on non-standard feature set level");
            let completed = builder.build();
            assert!(
                !completed
                    .core_shards
                    .iter()
                    .any(|path| path.as_str().contains("proprietary_tee_manager"))
            );
        }

        // 3. Product config TEE (e.g. GlobalPlatform) still processes on non-Standard
        // feature set levels.
        let mut context_utility = ConfigurationContext::default_for_tests();
        context_utility.board_config = &board_config;
        context_utility.feature_set_level = &FeatureSetLevel::Utility;
        context_utility.resource_dir =
            Utf8Path::from_path(resource_dir.path()).unwrap().to_path_buf();
        let global_platform_tee =
            Tee::GlobalPlatform(GlobalPlatformTee { clients: non_empty_tee_clients() });
        let mut builder = ConfigurationBuilderImpl::default();
        TeeConfig::define_configuration(
            &context_utility,
            &(&global_platform_tee, &Default::default(), &Default::default(), &Default::default()),
            &mut builder,
        )
        .expect("defining global platform TEE configuration on Utility level");
        let completed = builder.build();
        assert!(
            completed
                .core_shards
                .iter()
                .any(|path| path.as_str().contains("tee_manager.core_shard.cml"))
        );
    }

    fn populate_resource_dir(resource_dir: &Path) {
        std::fs::File::create(resource_dir.join("tee_manager.core_shard.cml")).unwrap();
        std::fs::File::create(
            resource_dir.join("proprietary_tee_manager.session.core_shard.cml.template"),
        )
        .unwrap();
        std::fs::File::create(
            resource_dir.join("proprietary_tee_manager.no_session.core_shard.cml.template"),
        )
        .unwrap();
    }
}
