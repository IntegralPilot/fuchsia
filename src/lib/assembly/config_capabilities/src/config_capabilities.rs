// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_util::NamedMap;
use camino::{Utf8Path, Utf8PathBuf};

use cml::translate::compile;
use cml::types::expose::{ContextExpose, ExposeFromRef};
use cml::{Availability, CompileOptions, ContextCapability, DocumentContext, OneOrMany};
use fidl::persist;
use fuchsia_pkg::{PackageBuilder, PackageManifest, RelativeTo};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::num::NonZeroU32;

use cml::types::common::synthetic_span;

/// The inner type of a vector configuration capability.
pub use cm_rust::ConfigNestedValueType;
/// The type of a configuration capability.
pub use cm_rust::ConfigValueType;

/// A collection of configuration capabilities.
/// The name is the capability name, and the Config struct contains the configuration type and value.
///
/// A None value corresponds to a capability that explicitly ends in "void", which is relevant for
/// availability optional config capabilities - when explicitly absent, they must route to "void".
pub type CapabilityNamedMap = NamedMap<String, Config>;

/// This represents a single configuration capability. It can easily be
/// converted into CML.
#[derive(Debug, PartialEq, Serialize)]
pub struct Config {
    #[serde(rename = "type")]
    type_: ConfigValueType,
    value: Value,
}

impl Config {
    /// Create a new configuration capability.
    pub fn new(type_: ConfigValueType, value: Value) -> Self {
        Config { type_, value }
    }

    /// Create a new boolean configuration capability.
    pub fn new_bool(value: bool) -> Self {
        Config::new(ConfigValueType::Bool, value.into())
    }

    /// Create a new uint64 configuration capability.
    pub fn new_uint64(value: u64) -> Self {
        Config::new(ConfigValueType::Uint64, value.into())
    }

    /// Create a new configuration capability whose source is "void". This is for any optional
    /// config capabilities that should be absent.
    pub fn new_void() -> Self {
        Config { type_: ConfigValueType::Bool, value: Value::Null }
    }

    /// The value of this configuration capability.
    pub fn value(&self) -> Value {
        self.value.clone()
    }

    fn as_capability(&self, name: cml::Name) -> Option<ContextCapability> {
        if self.value == Value::Null {
            return None;
        }
        Some(ContextCapability {
            config: Some(synthetic_span(name)),
            config_type: Some(synthetic_span((&self.type_).into())),
            config_max_size: self.get_max_size().map(synthetic_span),
            config_max_count: self.get_max_count().map(synthetic_span),
            config_element_type: self.get_nested_value_types().map(synthetic_span),
            value: Some(synthetic_span(self.value.clone())),
            ..Default::default()
        })
    }

    fn as_expose(&self, name: cml::Name) -> ContextExpose {
        if self.value == Value::Null {
            return ContextExpose {
                config: Some(synthetic_span(OneOrMany::One(name))),
                availability: Some(synthetic_span(Availability::Optional)),
                from: synthetic_span(OneOrMany::One(ExposeFromRef::Void)),
                ..Default::default()
            };
        }
        ContextExpose {
            config: Some(synthetic_span(OneOrMany::One(name))),
            from: synthetic_span(OneOrMany::One(ExposeFromRef::Self_)),
            ..Default::default()
        }
    }

    fn get_max_size(&self) -> Option<NonZeroU32> {
        self.type_.get_max_size().map(|s| NonZeroU32::new(s)).flatten()
    }

    fn get_nested_value_types(&self) -> Option<cml::ConfigNestedValueType> {
        self.type_.get_nested_type().map(|t| (&t).try_into().ok()).flatten()
    }

    fn get_max_count(&self) -> Option<NonZeroU32> {
        self.type_.get_max_count().map(|s| NonZeroU32::new(s)).flatten()
    }
}

/// Use the capabilities to build the `config` package, which consists of a single CML
/// at `meta/config.cm` containing all of the configuration capabilities.
pub fn build_config_capability_package(
    capabilities: CapabilityNamedMap,
    outdir: &Utf8Path,
) -> Result<(Utf8PathBuf, PackageManifest)> {
    std::fs::create_dir_all(&outdir).with_context(|| format!("creating directory {}", outdir))?;

    // Config capability packages built by assembly are never produced by
    // assembly tools from one Fuchsia release and then read by binaries from
    // another Fuchsia release.  Give them the platform ABI revision.
    let mut builder = PackageBuilder::new_platform_internal_package("config");

    let manifest_path = outdir.join("package_manifest.json");
    let metafar_path = outdir.join("meta.far");
    builder.manifest_path(&manifest_path);
    builder.manifest_blobs_relative_to(RelativeTo::File);

    let (cml_capabilities, exposes) = capabilities
        .into_iter()
        .map(|(name, config)| {
            let cml_name = cml::Name::new(name.as_str())
                .with_context(|| format! {"Invalid configuration name: {}", name})?;

            let cap = config.as_capability(cml_name.clone()).map(synthetic_span);
            let exp = synthetic_span(config.as_expose(cml_name));

            Ok((cap, exp))
        })
        .collect::<Result<std::vec::Vec<_>>>()?
        .into_iter()
        .unzip::<_, _, Vec<_>, Vec<_>>();

    let cml_capabilities: Vec<_> = cml_capabilities.into_iter().flatten().collect();

    let cml = DocumentContext {
        expose: Some(exposes),
        capabilities: Some(cml_capabilities),
        ..Default::default()
    };

    let out_data = compile(&cml, CompileOptions::default())
        .with_context(|| format!("compiling config capability CML"))?;

    let cm_name = format!("config.cm");
    let cm_path = outdir.join(&cm_name);
    let mut cm_file = std::fs::File::create(&cm_path)
        .with_context(|| format!("creating config capability CML: {cm_path}"))?;

    cm_file
        .write_all(&persist(&out_data)?)
        .with_context(|| format!("writing config capability CML: {cm_path}"))?;

    builder
        .add_file_to_far(format!("meta/{cm_name}"), &cm_path)
        .with_context(|| format!("adding file to config capability package: {cm_path}"))?;

    let manifest = builder
        .build(&outdir, metafar_path)
        .with_context(|| format!("building config capability package"))?;

    Ok((manifest_path, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use cm_rust::{ComponentDecl, ExposeDecl, ExposeSource, ExposeTarget};
    use fidl::unpersist;
    use fidl_fuchsia_component_decl::Component;
    use fuchsia_archive::Utf8Reader;
    use fuchsia_pkg::PackageName;
    use pretty_assertions::assert_eq;
    use std::fs::File;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[test]
    fn build() {
        let tmp = tempdir().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare the input
        let mut capabilities = CapabilityNamedMap::new("config capabilities");
        capabilities.insert(
            "fuchsia.config.MyConfig".to_string(),
            Config::new(ConfigValueType::Int64, 5.into()),
        );

        let (path, manifest) = build_config_capability_package(capabilities, &outdir).unwrap();

        // Assert the manifest is correct.
        assert_eq!(path, outdir.join("package_manifest.json"));
        let loaded_manifest = PackageManifest::try_load_from(path).unwrap();
        assert_eq!(manifest, loaded_manifest);
        assert_eq!(manifest.name(), &PackageName::from_str("config").unwrap());
        let blobs = manifest.into_blobs();
        assert_eq!(blobs.len(), 1);
        let blob = blobs.iter().find(|&b| &b.path == "meta/").unwrap();
        assert_eq!(blob.source_path, outdir.join("meta.far").to_string());

        // Assert the contents of the package are correct.
        let far_path = outdir.join("meta.far");
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(package, br#"{"name":"config","version":"0"}"#);
        let cm_bytes = far_reader.read_file("meta/config.cm").unwrap();
        let fidl_component_decl: Component = unpersist(&cm_bytes).unwrap();
        let component = ComponentDecl::try_from(fidl_component_decl).unwrap();
        assert_eq!(component.exposes.len(), 1);
        assert_matches!(&component.exposes[0], ExposeDecl::Config(cm_rust::ExposeConfigurationDecl {
            source: ExposeSource::Self_,
            source_name,
            target: ExposeTarget::Parent,
            ..
        }) => {
            assert_eq!(source_name, &cml::Name::new("fuchsia.config.MyConfig").unwrap());
        });
        assert_eq!(component.capabilities.len(), 1);
        assert_matches!(&component.capabilities[0], cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name,
            value,
        }) => {
            assert_eq!(name, &cml::Name::new("fuchsia.config.MyConfig").unwrap());
            assert_eq!(value, &cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Int64(5)));
        });
    }

    #[test]
    fn build_with_null_config() {
        let tmp = tempdir().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare the input
        let mut capabilities = CapabilityNamedMap::new("config capabilities");
        capabilities.insert(
            "fuchsia.config.MyConfig".to_string(),
            Config::new(ConfigValueType::Int64, None::<i64>.into()),
        );

        let (path, manifest) = build_config_capability_package(capabilities, &outdir).unwrap();

        // Assert the manifest is correct.
        assert_eq!(path, outdir.join("package_manifest.json"));
        let loaded_manifest = PackageManifest::try_load_from(path).unwrap();
        assert_eq!(manifest, loaded_manifest);
        assert_eq!(manifest.name(), &PackageName::from_str("config").unwrap());
        let blobs = manifest.into_blobs();
        assert_eq!(blobs.len(), 1);
        let blob = blobs.iter().find(|&b| &b.path == "meta/").unwrap();
        assert_eq!(blob.source_path, outdir.join("meta.far").to_string());

        // Assert the contents of the package are correct.
        let far_path = outdir.join("meta.far");
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(package, br#"{"name":"config","version":"0"}"#);
        let cm_bytes = far_reader.read_file("meta/config.cm").unwrap();
        let fidl_component_decl: Component = unpersist(&cm_bytes).unwrap();
        let component = ComponentDecl::try_from(fidl_component_decl).unwrap();

        // The null config creates an expose from Void.
        assert_eq!(component.exposes.len(), 1);
        assert_matches!(&component.exposes[0], ExposeDecl::Config(cm_rust::ExposeConfigurationDecl {
            source: ExposeSource::Void,
            source_name,
            target: ExposeTarget::Parent,
            availability: cm_rust::Availability::Optional,
            ..
        }) => {
            assert_eq!(source_name, &cml::Name::new("fuchsia.config.MyConfig").unwrap());
        });

        // The null config doesn't create a capability.
        assert_eq!(component.capabilities.len(), 0);
    }
}
