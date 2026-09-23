// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, bail};
use assembly_components::ComponentBuilder;
use assembly_config_schema::PackageSet;
use assembly_config_schema::assembly_input_bundle::{
    CompiledComponentDefinition, CompiledPackageDefinition,
};
use assembly_constants::FileEntry;
use assembly_tool::Tool;
use assembly_util::{DuplicateKeyError, InsertUniqueExt, MapEntry};
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_pkg::{PackageBuilder, RelativeTo};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct CompiledComponentProperties {
    /// CML shards for the components to compile and add to the package.
    /// First by component name, then by shard filename.
    pub shards: BTreeMap<String, Utf8PathBuf>,
    /// The CMC features required for compiling this component.
    pub cmc_features: BTreeSet<String>,
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct CompiledPackageBuilder {
    /// Name of the package to compile.
    pub name: String,

    /// CML shards for the components to compile and add to the package.
    /// First by component name, then by shard filename.
    pub components: BTreeMap<String, CompiledComponentProperties>,

    /// Non-component files to add to the package.
    pub contents: Vec<FileEntry<String>>,

    /// Whether the contents of this package should go into bootfs.
    /// Gated by allowlist -- please use this as a base package if possible.
    ///
    /// If never specified, the default value of 'false' will be used.
    pub bootfs_package: Option<bool>,

    /// Component includes dir for this package.  There can be only one.
    pub includes_dir: Option<Utf8PathBuf>,
}

/// Builds `CompiledPackageDefinition`s which are specified for Assembly
/// to create into packages.
impl CompiledPackageBuilder {
    pub fn new(name: impl Into<String>) -> CompiledPackageBuilder {
        CompiledPackageBuilder { name: name.into(), ..Default::default() }
    }

    /// Add a package definition to the compiled package
    ///
    /// # Arguments
    ///
    /// * entry -- a reference to a [CompiledPackageDefinition].
    /// * bundle_dir -- location of this [CompiledPackageDefinition]'s AIB.
    ///     The locations of the files in the [CompiledPackageDefinition]
    ///     are defined relative to the bundle.
    pub fn add_package_def(
        &mut self,
        entry: &CompiledPackageDefinition,
        bundle_dir: impl AsRef<Utf8Path>,
    ) -> Result<&mut Self> {
        let name = entry.name.to_string();

        if name != self.name {
            bail!(
                "Builder name '{}' does not match CompiledPackageDefinition name '{name}'",
                self.name
            );
        }

        match self.bootfs_package {
            Some(existing_value) => {
                if entry.bootfs_package != existing_value {
                    bail!(
                        "CompiledPackageDefinitions are inconsistent about if '{name}' is a bootfs package."
                    );
                }
            }
            None => self.bootfs_package = Some(entry.bootfs_package),
        }

        // Add the static package contents
        for FileEntry { source, destination } in &entry.contents {
            let rebased_entry = FileEntry {
                source: bundle_dir.as_ref().join(source),
                destination: destination.clone(),
            };
            self.contents.push(rebased_entry);
        }

        // Set the component includes dir for this package, if not already set.
        if !entry.includes.is_empty() {
            if let Some(existing_path) =
                self.includes_dir.replace(bundle_dir.as_ref().join("compiled_packages/include"))
            {
                bail!(
                    "There can be only one AIB that provides a cml includes dir for a package, it was already set to: {existing_path}"
                );
            }
        }

        for CompiledComponentDefinition { component_name, shards, cmc_features } in
            &entry.components
        {
            let component_properties = self.components.entry(component_name.clone()).or_default();

            for shard_path in shards {
                let filename = shard_path.file_name().ok_or_else(|| {
                    anyhow!("The component shard path does not have a filename: {}", shard_path)
                })?;
                component_properties.shards
                .try_insert_unique(MapEntry(
                    filename.to_string(),
                    shard_path.clone(),
                ))
                .map_err(|shard| {
                    anyhow!(
                        "Duplicate component shard found for {}/meta/{}.cm: {} \n          {}\n        and\n          {}",
                        self.name,
                        component_name,
                        shard.key(),
                        shard.previous_value(),
                        shard.new_value(),
                    )
                })?;
                component_properties.cmc_features.extend(cmc_features.iter().cloned());
            }
        }

        Ok(self)
    }

    fn build_component<T: AsRef<Utf8Path>>(
        shards: impl Iterator<Item = T>,
        cmc_features: impl Iterator<Item: Into<String>>,
        cmc_tool: &dyn Tool,
        component_includes_dir: &Option<Utf8PathBuf>,
        component_name: &String,
        outdir: impl AsRef<Utf8Path>,
    ) -> Result<Utf8PathBuf> {
        let mut component_builder = ComponentBuilder::new(component_name);

        for cml_shard in shards {
            component_builder.add_shard(cml_shard)?;
        }
        for feature in cmc_features {
            component_builder.add_cmc_feature(feature);
        }

        component_builder.build(&outdir, component_includes_dir, cmc_tool)
    }

    /// Build the compiled package as a package
    fn build_package(
        &self,
        cmc_tool: &dyn Tool,
        outdir: impl AsRef<Utf8Path>,
    ) -> Result<Utf8PathBuf> {
        let outdir = outdir.as_ref().join(&self.name);

        // Assembly-compiled packages are never produced by assembly tools from
        // one Fuchsia release and then read by binaries from another Fuchsia
        // release. Give them the platform ABI revision.
        let mut package_builder = PackageBuilder::new_platform_internal_package(&self.name);
        package_builder.repository("fuchsia.com");

        for (component_name, properties) in &self.components {
            let CompiledComponentProperties { shards, cmc_features } = properties;
            let component_manifest_path = Self::build_component(
                shards.values(),
                cmc_features.iter(),
                cmc_tool,
                &self.includes_dir,
                component_name,
                &outdir,
            )
            .with_context(|| format!("building component {component_name}"))?;
            let component_manifest_file_name =
                component_manifest_path.file_name().context("component file name")?;

            let component_path = format!("meta/{component_manifest_file_name}");
            package_builder.add_file_to_far(component_path, component_manifest_path)?;
        }

        for entry in &self.contents {
            package_builder.add_file_as_blob(&entry.destination, &entry.source)?;
        }

        let package_manifest_path = outdir.join("package_manifest.json");
        package_builder.manifest_path(&package_manifest_path);
        package_builder.manifest_blobs_relative_to(RelativeTo::File);
        let metafar_path = outdir.join(format!("{}.far", self.name));
        package_builder.build(&outdir, metafar_path).context("building package")?;

        Ok(package_manifest_path)
    }

    /// Build the components for the package and build the package
    ///
    /// Returns a path to the package manifest, and whether the package should be placed in bootfs
    /// or the base package set.
    pub fn build(
        self,
        cmc_tool: &dyn Tool,
        outdir: impl AsRef<Utf8Path>,
    ) -> Result<(Utf8PathBuf, PackageSet)> {
        let package_set = if self.bootfs_package.unwrap_or_default() {
            PackageSet::Bootfs
        } else {
            PackageSet::Base
        };
        Ok((self.build_package(cmc_tool, outdir)?, package_set))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_constants::CompiledPackageDestination;
    use assembly_constants::TestCompiledPackageDestination::ForTest;
    use assembly_tool::ToolProvider;
    use assembly_tool::testing::FakeToolProvider;
    use fuchsia_archive::Utf8Reader;
    use fuchsia_pkg::PackageManifest;
    use std::fs::File;
    use tempfile::TempDir;

    // NOTE: This string matches the string representation of
    // CompiledPackageDestination::Test.
    const FOR_TEST_PACKAGE_NAME: &'static str = "for-test";

    #[test]
    fn add_package_def_appends_entries_to_builder() {
        let mut compiled_package_builder = CompiledPackageBuilder::new(FOR_TEST_PACKAGE_NAME);
        let outdir_tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(outdir_tmp.path()).unwrap();
        make_test_package_and_components(outdir);

        compiled_package_builder
            .add_package_def(
                &CompiledPackageDefinition {
                    name: CompiledPackageDestination::Test(ForTest),
                    components: vec![
                        CompiledComponentDefinition {
                            component_name: "component1".into(),
                            shards: vec![outdir.join("cml1").into()],
                            cmc_features: vec![],
                        },
                        CompiledComponentDefinition {
                            component_name: "component2".into(),
                            shards: vec![outdir.join("cml2").into()],
                            cmc_features: vec![],
                        },
                    ],
                    contents: vec![FileEntry {
                        source: outdir.join("file1"),
                        destination: "file1".into(),
                    }],
                    includes: Default::default(),
                    bootfs_package: Default::default(),
                },
                outdir,
            )
            .unwrap()
            .add_package_def(
                &CompiledPackageDefinition {
                    name: CompiledPackageDestination::Test(ForTest),
                    components: vec![CompiledComponentDefinition {
                        component_name: "component2".into(),
                        shards: vec![outdir.join("shard1").into()],
                        cmc_features: vec![],
                    }],
                    contents: Default::default(),
                    includes: Default::default(),
                    bootfs_package: Default::default(),
                },
                outdir,
            )
            .unwrap();

        assert_eq!(
            compiled_package_builder,
            CompiledPackageBuilder {
                name: FOR_TEST_PACKAGE_NAME.into(),
                components: BTreeMap::from([
                    (
                        "component1".into(),
                        CompiledComponentProperties {
                            shards: BTreeMap::from([("cml1".into(), outdir.join("cml1"))]),
                            cmc_features: BTreeSet::new(),
                        }
                    ),
                    (
                        "component2".into(),
                        CompiledComponentProperties {
                            shards: BTreeMap::from([
                                ("cml2".into(), outdir.join("cml2")),
                                ("shard1".into(), outdir.join("shard1"))
                            ]),
                            cmc_features: BTreeSet::new(),
                        }
                    )
                ]),
                contents: vec![FileEntry {
                    source: outdir.join("file1"),
                    destination: "file1".into()
                }],
                includes_dir: None,
                bootfs_package: Some(false),
            }
        );
    }

    #[test]
    fn build_builds_package() {
        let mut compiled_package_builder = CompiledPackageBuilder::new(FOR_TEST_PACKAGE_NAME);
        let tools = FakeToolProvider::default();
        let outdir_tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(outdir_tmp.path()).unwrap();
        make_test_package_and_components(outdir);

        compiled_package_builder
            .add_package_def(
                &CompiledPackageDefinition {
                    name: CompiledPackageDestination::Test(ForTest),
                    components: vec![
                        CompiledComponentDefinition {
                            component_name: "component1".into(),
                            shards: vec!["cml1".into()],
                            cmc_features: vec![],
                        },
                        CompiledComponentDefinition {
                            component_name: "component2".into(),
                            shards: vec!["cml2".into()],
                            cmc_features: vec![],
                        },
                    ],
                    contents: vec![FileEntry {
                        source: outdir.join("file1"),
                        destination: "file1".into(),
                    }],
                    includes: Default::default(),
                    bootfs_package: Default::default(),
                },
                outdir,
            )
            .unwrap()
            .add_package_def(
                &CompiledPackageDefinition {
                    name: CompiledPackageDestination::Test(ForTest),
                    components: vec![CompiledComponentDefinition {
                        component_name: "component2".into(),
                        shards: vec!["shard1".into()],
                        cmc_features: vec![],
                    }],
                    contents: Default::default(),
                    includes: Default::default(),
                    bootfs_package: Default::default(),
                },
                outdir,
            )
            .unwrap();

        compiled_package_builder.build(tools.get_tool("cmc").unwrap().as_ref(), outdir).unwrap();

        let compiled_package_file = File::open(outdir.join("for-test/for-test.far")).unwrap();
        let mut far_reader = Utf8Reader::new(&compiled_package_file).unwrap();
        let manifest_path = outdir.join("for-test/package_manifest.json");
        assert_far_contents_eq(
            &mut far_reader,
            "meta/contents",
            "file1=b5209759e76a8343c45b8c7abad13a1f0609512865ee7f7f5533212d8ab334dc\n",
        );
        assert_far_contents_eq(&mut far_reader, "meta/component1.cm", "component fake contents");
        let package_manifest = PackageManifest::try_load_from(manifest_path).unwrap();
        assert_eq!(package_manifest.name().as_ref(), FOR_TEST_PACKAGE_NAME);
    }

    #[test]
    fn add_package_def_with_wrong_name_returns_err() {
        let mut compiled_package_builder = CompiledPackageBuilder::new("bar");

        let result = compiled_package_builder.add_package_def(
            &CompiledPackageDefinition {
                name: CompiledPackageDestination::Test(ForTest),
                components: Default::default(),
                contents: Default::default(),
                includes: Default::default(),
                bootfs_package: Default::default(),
            },
            "assembly/input/bundle/path/compiled_packages/include",
        );

        assert!(result.is_err());
    }

    fn assert_far_contents_eq(
        far_reader: &mut Utf8Reader<&File>,
        path: &str,
        expected_contents: &str,
    ) {
        let contents = far_reader.read_file(path).unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        assert_eq!(contents, expected_contents);
    }

    fn make_test_package_and_components(outdir: &Utf8Path) {
        // We're mocking the component compiler but we are
        // using the real packaging library.
        // Write the contents of the package.
        let file1 = outdir.join("file1");
        std::fs::write(file1, "file1 contents").unwrap();
        // Write the expected output component files since the component
        // compiler is mocked.
        let component1_dir = outdir.join("for-test/component1");
        let component2_dir = outdir.join("for-test/component2");
        std::fs::create_dir_all(&component1_dir).unwrap();
        std::fs::create_dir_all(&component2_dir).unwrap();
        std::fs::write(component1_dir.join("component1.cm"), "component fake contents").unwrap();
        std::fs::write(component2_dir.join("component2.cm"), "component fake contents").unwrap();
    }
}
