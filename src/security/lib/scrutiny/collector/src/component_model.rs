// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::package::ROOT_RESOURCE;
use anyhow::{Context, Result, anyhow};
use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use cm_config::RuntimeConfig;
use cm_fidl_analyzer::component_model::{DynamicConfig, ModelBuilderForAnalyzer};
use cm_rust::{CapabilityTypeName, ComponentDecl, FidlIntoNative};
use cm_types::{Name, Url};
use config_encoder::ConfigFields;
use fidl::unpersist;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_internal as component_internal;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use log::{error, info, warn};
use moniker::Moniker;
use scrutiny_collection::core::{Components, CoreDataDeps, ManifestData, Manifests};
use scrutiny_collection::model::DataModel;
use scrutiny_collection::v2_component_model::V2ComponentModel;
use scrutiny_collection::zbi::Zbi;
use serde::{Deserialize, Serialize};
use serde_json5::from_reader;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

// The default root component URL used to identify the root instance of the component model
// unless the RuntimeConfig specifies a different root URL.
pub static DEFAULT_ROOT_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::new(
        &fuchsia_url::boot::AbsoluteComponentUrl::new(
            Some("root".parse().expect("valid url path")),
            ROOT_RESOURCE.parse().expect("valid url resource"),
        )
        .to_string(),
    )
    .unwrap()
});

// The path to the runtime config in bootfs.
pub const DEFAULT_CONFIG_PATH: &str = "config/component_manager";

// The name of the ELF runner.
pub const ELF_RUNNER_NAME: &str = "elf";
// The name of the RealmBuilder runner.
pub const REALM_BUILDER_RUNNER_NAME: &str = "realm_builder";

#[derive(Deserialize, Serialize)]
pub struct DynamicComponent {
    pub url: AbsoluteComponentUrl,
    pub environment: Option<Name>,
}

#[derive(Deserialize, Serialize)]
pub struct ComponentTreeConfig {
    pub dynamic_components: HashMap<Moniker, DynamicComponent>,
    #[serde(default)]
    /// Defines the capabilities that are OK to be present in dynamic dictionaries exposed by
    /// components. Maps from moniker to a map from dictionary name to capabilities that may be
    /// routed from that dynamic dictionary.
    pub dynamic_dictionaries: HashMap<Moniker, HashMap<Name, DictionaryCapabilities>>,
}

#[derive(Deserialize, Serialize)]
pub enum DictionaryCapabilities {
    #[serde(rename = "protocols")]
    Protocols(Vec<Name>),
    #[serde(rename = "configs")]
    Configs(Vec<Name>),
}

#[derive(Default)]
pub struct V2ComponentModelDataCollector {}

impl V2ComponentModelDataCollector {
    pub fn new() -> Self {
        Self {}
    }

    fn get_decls(
        &self,
        model: &Arc<DataModel>,
    ) -> Result<HashMap<Url, (ComponentDecl, Option<ConfigFields>)>> {
        let mut decls = HashMap::new();
        let mut urls = HashMap::new();

        let components =
            model.get::<Components>().context("Unable to retrieve components from the model")?;
        for component in components.entries.iter() {
            urls.insert(component.id, component.url.clone());
        }

        for manifest in model
            .get::<Manifests>()
            .context("Unable to retrieve manifests from the model")?
            .entries
            .iter()
        {
            let ManifestData { cm_base64, cvf_bytes } = &manifest.manifest;

            match urls.remove(&manifest.component_id) {
                Some(url) => {
                    let result: Result<fdecl::Component, fidl::Error> = unpersist(
                        &BASE64_STANDARD
                            .decode(&cm_base64)
                            .context("Unable to decode base64 v2 manifest")?,
                    );
                    match result {
                        Ok(decl) => {
                            let decl: cm_rust::ComponentDecl = decl.fidl_into_native();
                            let config = if let Some(schema) = &decl.config {
                                if let Some(cvf_bytes) = cvf_bytes.as_ref() {
                                    let values_data =
                                        unpersist::<fdecl::ConfigValuesData>(cvf_bytes)
                                            .context("decoding config values")?
                                            .fidl_into_native();
                                    // TODO(https://fxbug.dev/42077231) collect static parent overrides
                                    let resolved = ConfigFields::resolve(schema, values_data, None)
                                        .context("resolving configuration")?;
                                    Some(resolved)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            decls.insert(url, (decl, config));
                        }
                        Err(err) => {
                            error!(
                                err:%,
                                url:%;
                                "Manifest for component is corrupted"
                            );
                        }
                    }
                }
                None => {
                    return Err(anyhow!(
                        "No component URL found for v2 component with id {}",
                        manifest.component_id
                    ));
                }
            }
        }
        Ok(decls)
    }

    fn get_runtime_config(&self, config_path: &str, zbi: &Zbi) -> Result<RuntimeConfig> {
        match zbi.bootfs_files.bootfs_files.get(config_path) {
            Some(config_data) => Ok(RuntimeConfig::try_from(
                unpersist::<component_internal::Config>(&config_data)
                    .context("Unable to decode runtime config")?,
            )
            .context("Unable to parse runtime config")?),
            None => Err(anyhow!("file {} not found in bootfs", config_path)),
        }
    }

    fn get_component_id_index(
        &self,
        index_path: Option<&str>,
        zbi: &Zbi,
    ) -> Result<component_id_index::Index> {
        match index_path {
            Some(path) => {
                let split: Vec<&str> = path.split_inclusive("/").collect();
                if split.as_slice()[..2] == ["/", "boot/"] {
                    let remainder = split[2..].join("");
                    match zbi.bootfs_files.bootfs_files.get(&remainder) {
                        Some(index_data) => {
                            let fidl_index =
                                unpersist::<component_internal::ComponentIdIndex>(index_data)
                                    .context(
                                        "Unable to decode component ID index from persistent FIDL",
                                    )?;
                            let index: component_id_index::Index = fidl_index.try_into().context(
                                "Unable to create internal index for component ID index from FIDL",
                            )?;
                            Ok(index)
                        }
                        None => Err(anyhow!("file {} not found in bootfs", remainder)),
                    }
                } else {
                    Err(anyhow!("Unable to parse component ID index file path {}", path))
                }
            }
            None => Ok(component_id_index::Index::default()),
        }
    }

    // Dynamic config may be represented across multiple files, e.g. platform vs product.
    fn load_dynamic_config(component_tree_config_paths: &Vec<PathBuf>) -> Result<DynamicConfig> {
        if component_tree_config_paths.len() == 0 {
            return Ok(DynamicConfig::default());
        };

        let mut dynamic_components = HashMap::new();
        let mut dynamic_dictionaries = HashMap::new();

        for config_path in component_tree_config_paths {
            let mut component_tree_config_file = File::open(config_path)
                .context("Failed to open component tree configuration file")?;
            let component_tree_config: ComponentTreeConfig =
                from_reader(&mut component_tree_config_file)
                    .context("Failed to parse component tree configuration file")?;

            for (moniker, dynamic_component) in component_tree_config.dynamic_components {
                dynamic_components
                    .insert(moniker, (dynamic_component.url, dynamic_component.environment));
            }

            for (moniker, dynamic_dictionary) in component_tree_config.dynamic_dictionaries {
                let map: &mut HashMap<Name, Vec<(CapabilityTypeName, Name)>> =
                    dynamic_dictionaries.entry(moniker).or_default();
                for (dictionary_name, capabilities) in dynamic_dictionary {
                    map.entry(dictionary_name).or_default().extend(match capabilities {
                        DictionaryCapabilities::Protocols(protocols) => Box::new(
                            protocols
                                .into_iter()
                                .map(|protocol_name| (CapabilityTypeName::Protocol, protocol_name)),
                        )
                            as Box<dyn Iterator<Item = (CapabilityTypeName, Name)>>,
                        DictionaryCapabilities::Configs(configs) => Box::new(
                            configs
                                .into_iter()
                                .map(|config_name| (CapabilityTypeName::Config, config_name)),
                        )
                            as Box<dyn Iterator<Item = (CapabilityTypeName, Name)>>,
                    });
                }
            }
        }

        Ok(DynamicConfig { components: dynamic_components, dictionaries: dynamic_dictionaries })
    }

    pub fn collect(&self, model: Arc<DataModel>) -> Result<()> {
        let builder = ModelBuilderForAnalyzer::new(DEFAULT_ROOT_URL.clone());

        let decls_by_url = self.get_decls(&model)?;

        let zbi = &model.get::<Zbi>().context("Unable to find the zbi model.")?;
        let runtime_config = self.get_runtime_config(DEFAULT_CONFIG_PATH, &zbi).context(
            format!("Unable to get the runtime config at path {:?}", DEFAULT_CONFIG_PATH),
        )?;
        let component_id_index = self.get_component_id_index(
            runtime_config.component_id_index_path.as_ref().map(|path| path.as_str()),
            &zbi,
        )?;

        info!(
            total = decls_by_url.len();
            "V2ComponentModelDataCollector: Found v2 component declarations",
        );

        let dynamic_config =
            Self::load_dynamic_config(&model.config().component_tree_config_paths)?;
        let build_result = builder.build_with_dynamic_config(
            dynamic_config,
            decls_by_url,
            Arc::new(runtime_config),
            Arc::new(component_id_index),
        );

        for err in build_result.errors.iter() {
            warn!(err:%; "V2ComponentModelDataCollector");
        }

        match build_result.model {
            Some(component_model) => {
                info!(
                    total_instances = component_model.len();
                    "V2ComponentModelDataCollector: Built v2 component model"
                );
                let core_deps_collection: Arc<CoreDataDeps> = model.get().map_err(|err| {
                    anyhow!("Failed to read core data deps for v2 component model data: {}", err)
                })?;
                let deps = core_deps_collection.deps.clone();
                model
                    .set(V2ComponentModel::new(deps, component_model, build_result.errors))
                    .map_err(|err| {
                        anyhow!("Failed to store v2 component model in data model: {}", err)
                    })?;
                Ok(())
            }
            None => Err(anyhow!("Failed to build v2 component model")),
        }
    }
}
