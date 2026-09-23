// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, bail};
use assembly_config_capabilities::CapabilityNamedMap;
use assembly_config_schema::assembly_input_bundle::{
    AssemblyInputBundle, CompiledPackageDefinition,
};
use assembly_config_schema::developer_overrides::DeveloperOnlyOptions;
use assembly_container::AssemblyContainer;
use assembly_platform_artifacts::PlatformArtifacts;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use assembly_config_schema::platform_settings::icu_config::{ICU_CONFIG_INFO, ICUMap, Revision};
use assembly_config_schema::{BoardConfig, BuildType, FeatureSetLevel, ICUConfig};
use assembly_constants::{
    BoardFeature, BootfsDestination, CompiledPackageDestination, FileEntry, KernelArg,
    PackageSetDestination,
};
use assembly_named_file_map::NamedFileMap;
use assembly_util::NamedMap;

/// A trait for subsystems to implement to provide the configuration for their
/// subsystem's components.
pub(crate) trait DefineSubsystemConfiguration<T> {
    /// Given the feature_set_level and build_type, along with the configuration
    /// schema for the subsystem, add its configuration to the builder.
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        subsystem_config: &T,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()>;
}

/// This provides the context that's passed to each subsystem's configuration
/// module.  These are the fields from the
/// `assembly_config_schema::platform_settings::PlatformSettings` struct that are
/// available to all subsystems to use to derive their configuration from.
#[derive(Debug)]
pub(crate) struct ConfigurationContext<'a> {
    pub feature_set_level: &'a FeatureSetLevel,
    pub build_type: &'a BuildType,
    pub board_config: &'a BoardConfig,
    pub gendir: Utf8PathBuf,
    pub resource_dir: Utf8PathBuf,
    pub developer_only_options: Option<&'a DeveloperOnlyOptions>,
}

impl ConfigurationContext<'_> {
    /// Ensure that the configuration context matches the given set of
    /// build-types and feature-set-levels.
    ///
    /// Note that this is not a mechanism to enforce policy; they simply make configuration errors
    /// more obvious to users by providing a clear error message.
    ///
    /// Returns an error if they do not, and Ok(()) if they do.
    pub fn ensure_build_type_and_feature_set_level(
        &self,
        build_types: &[BuildType],
        feature_set_levels: &[FeatureSetLevel],
        item_name: &str,
    ) -> anyhow::Result<()> {
        self.ensure_build_type(build_types, item_name)?;
        self.ensure_feature_set_level(feature_set_levels, item_name)
    }

    /// Ensure that the configuration context matches the given set of
    /// build-types.
    ///
    /// Note that this is not a mechanism to enforce policy; they simply make configuration errors
    /// more obvious to users by providing a clear error message.
    ///
    /// Returns an error if they do not, and Ok(()) if they do.
    pub fn ensure_build_type(
        &self,
        build_types: &[BuildType],
        item_name: &str,
    ) -> anyhow::Result<()> {
        if !build_types.contains(self.build_type) {
            bail!(
                "{} can only be enabled on the following build types, not '{}': [{}]",
                item_name,
                self.build_type,
                build_types.iter().map(|b| b.to_string()).collect::<Vec<String>>().join(", ")
            );
        }
        Ok(())
    }

    /// Ensure that the configuration context matches the give set of
    /// feature-set-levels.
    ///
    /// Note that this is not a mechanism to enforce policy; they simply make configuration errors
    /// more obvious to users by providing a clear error message.
    ///
    /// Returns an error if they do not, and Ok(()) if they do.
    pub fn ensure_feature_set_level(
        &self,
        feature_set_levels: &[FeatureSetLevel],
        item_name: &str,
    ) -> anyhow::Result<()> {
        if !feature_set_levels.contains(self.feature_set_level) {
            bail!(
                "{} can only be enabled on the following feature sets, not '{}': [{}]",
                item_name,
                self.feature_set_level,
                feature_set_levels
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        Ok(())
    }
}

/// A struct for collecting multiple kinds of platform configuration.
///
/// Subsystem configuration structs use this builder to add their configuration
/// to the assembly.
///
/// Usage:
/// ```
/// use crate::subsystems::prelude::*;
/// let icu_cfg = Default::default();
/// let builder = ConfigurationBuilder::new(&icu_cfg);
/// builder.platform_bundle("wlan")?;
///
/// // to set a single field on a single component in a package:
/// builder.package("wlancfg").component("wlancfg").field("my_key", "some_value");
///
/// // to set a single field on multiple components in the same package:
/// let mut swd_package = builder.package("swd");
/// swd_package.component("meta/foo.cm").field("some_key", "some_value");
/// swd_package.component("meta/bar.cm").field("some_key", "some_value");
///
/// // to set multiple fieds on the same component:
/// swd_package.component("meta/mine.cm")
///   .field("a", "value1")
///
pub(crate) trait ConfigurationBuilder {
    /// Add all the AIBs that should be auto-included based on feature set level and build type.
    fn add_auto_include_bundles(
        &mut self,
        platform_artifacts: &PlatformArtifacts,
        feature_set_level: &FeatureSetLevel,
        build_type: &BuildType,
    );

    /// Add a platform assembly input bundle that should be included in the
    /// assembled platform.
    fn platform_bundle(&mut self, name: &str) -> Result<()>;

    /// Add an ICU-flavored platform assembly input bundle that should
    /// be included in the assembled platform, with a specified ICU configuration.
    ///
    /// If `icu_config` is `None`, an unflavored version of the bundle is used,
    /// as if [platform_bundle] was called.
    fn icu_platform_bundle(&mut self, name: &str) -> Result<()>;

    /// Add configuration for items in bootfs.
    fn bootfs(&mut self) -> &mut dyn BootfsConfigBuilder;

    /// Add configuration for a named package in one of the package sets.
    fn package(&mut self, name: &str) -> &mut dyn PackageConfigBuilder;

    /// Create a new domain config package.
    fn add_domain_config(&mut self, name: PackageSetDestination) -> &mut dyn DomainConfigBuilder;

    /// Add a core shard.
    fn core_shard(&mut self, path: &Utf8PathBuf);

    /// Sets a configuration capability.
    fn set_config_capability(
        &mut self,
        name: &str,
        config: assembly_config_capabilities::Config,
    ) -> Result<()>;

    /// Add a kernel command line arg that should be included in the
    /// assembled platform.
    fn kernel_arg(&mut self, arg: KernelArg);

    /// Packages compiled by subsystems to include in the assembled product.
    /// Example: the trusted apps package which is configured by the product
    /// configuration.
    fn compiled_package(
        &mut self,
        destination: CompiledPackageDestination,
        definition: CompiledPackageDefinition,
    ) -> Result<&mut CompiledPackageDefinition>;
}

/// The interface for specifying the configuration to provide for bootfs.
pub(crate) trait BootfsConfigBuilder {
    /// Add a file to bootfs.
    fn file(
        &mut self,
        file_entry: FileEntry<BootfsDestination>,
    ) -> Result<&mut dyn BootfsConfigBuilder>;
}

/// The interface for specifying the configuration to provide for a package.
pub(crate) trait PackageConfigBuilder {
    /// Add configuration to the builder for a component within a package.
    fn component(&mut self, pkg_path: &str) -> Result<&mut dyn ComponentConfigBuilder>;

    /// Add a config data file to the package in the builder.
    fn config_data(
        &mut self,
        file_entry: FileEntry<String>,
    ) -> Result<&mut dyn PackageConfigBuilder>;

    /// Add a set of optionally-present files with their destination paths
    fn optional_config_data_files(
        &mut self,
        paths: Vec<(&Option<Utf8PathBuf>, &str)>,
    ) -> Result<&mut dyn PackageConfigBuilder>;
}

/// The interface for building a domain config.
pub(crate) trait DomainConfigBuilder {
    /// Add a directory to the domain config which can hold config resources.
    fn directory(&mut self, name: &str) -> &mut dyn DomainConfigDirectoryBuilder;

    /// Avoid exposing the directory via capability routing.
    fn skip_expose(&mut self) -> &mut dyn DomainConfigBuilder;
}

/// The interface for specifying the config files to add to a domain config package directory.
pub(crate) trait DomainConfigDirectoryBuilder {
    /// Add a file to the directory.
    fn entry(
        &mut self,
        file_entry: FileEntry<String>,
    ) -> Result<&mut dyn DomainConfigDirectoryBuilder>;

    /// Add a file to the directory using the contents provided.
    fn entry_from_contents(
        &mut self,
        destination: &str,
        contents: &str,
    ) -> Result<&mut dyn DomainConfigDirectoryBuilder>;

    /// Add a file to the directory using the binary contents provided.
    fn entry_from_binary_contents(
        &mut self,
        destination: &str,
        contents: Vec<u8>,
    ) -> Result<&mut dyn DomainConfigDirectoryBuilder>;
}

/// The interface for specifying the configuration to provide for a component.
pub(crate) trait ComponentConfigBuilder {
    /// Add a value for a Structured Configuration field for a given component.
    fn field_value(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<&mut dyn ComponentConfigBuilder>;
}

/// An extension trait that allows the field value to be passed without calling
/// `.into()` at the call-site.
pub(crate) trait ComponentConfigBuilderExt {
    /// Add a value for a Structured Configuration field for a given component.
    fn field(
        &mut self,
        key: &str,
        value: impl Into<serde_json::Value>,
    ) -> Result<&mut dyn ComponentConfigBuilder>;
}

impl ComponentConfigBuilderExt for &mut dyn ComponentConfigBuilder {
    fn field(
        &mut self,
        key: &str,
        value: impl Into<serde_json::Value>,
    ) -> Result<&mut dyn ComponentConfigBuilder> {
        ComponentConfigBuilder::field_value(*self, key, value.into())
    }
}

/// The in-progress builder, which hides its state.
pub(crate) struct ConfigurationBuilderImpl {
    /// The Assembly Input Bundles to add.
    bundles: BTreeSet<String>,

    /// Bundles that have `auto_include_in` set, and thus cannot be manually included.
    auto_includable_bundles: BTreeSet<String>,

    /// BootFS configuration.
    bootfs: BootfsConfig,

    /// Per-package configuration.
    package_configs: PackageConfigs,

    /// The domain config packages to add.
    domain_configs: DomainConfigs,

    /// The desired ICU configuration, used to configure subsystems that are
    /// ICU-flavor aware.
    ///
    /// If not set, use the unflavored version of the component.
    icu_config: ICUConfig,

    /// The core shards to add.
    core_shards: Vec<Utf8PathBuf>,

    /// The configuration capabilities to add (or route to "void").
    configuration_capabilities: CapabilityNamedMap,

    /// The Kernel Commandline Arguments to add.
    kernel_args: BTreeSet<String>,

    /// Packages compiled by assembly subsystems to include in the assembled product.
    compiled_packages: BTreeMap<CompiledPackageDestination, CompiledPackageDefinition>,
}

#[cfg(test)]
impl Default for ConfigurationBuilderImpl {
    /// Bypasses the need to define an [ICUConfig] in tests.
    fn default() -> Self {
        ConfigurationBuilderImpl::new(ICUConfig::default())
    }
}

impl ConfigurationBuilderImpl {
    /// Create a new builder. `icu_config` is a configuration for the
    /// ICU library.
    pub fn new(icu_config: ICUConfig) -> Self {
        Self {
            bundles: BTreeSet::default(),
            bootfs: BootfsConfig::default(),
            package_configs: PackageConfigs::new("package configs"),
            domain_configs: DomainConfigs::new("domain configs"),
            icu_config,
            core_shards: Vec::new(),
            configuration_capabilities: CapabilityNamedMap::new("configuration capabilities"),
            kernel_args: BTreeSet::default(),
            compiled_packages: BTreeMap::new(),
            auto_includable_bundles: BTreeSet::new(),
        }
    }

    /// Convert the builder into the completed configuration that can be used
    /// to create the configured platform itself.
    pub fn build(self) -> CompletedConfiguration {
        let Self {
            bundles,
            auto_includable_bundles: _,
            bootfs,
            package_configs,
            domain_configs,
            icu_config: _,
            core_shards,
            configuration_capabilities,
            kernel_args,
            compiled_packages,
        } = self;
        CompletedConfiguration {
            bundles,
            bootfs,
            package_configs,
            domain_configs,
            core_shards,
            configuration_capabilities,
            kernel_args,
            compiled_packages,
        }
    }
}

/// The struct containing the resultant configuration to apply to the platform.
pub struct CompletedConfiguration {
    /// The list of the Platform Assembly Input Bundles to add.
    ///
    /// This is a list of Platform AIBs by name (not path).
    ///
    pub bundles: BTreeSet<String>,

    /// Configuration for items in bootfs.
    pub bootfs: BootfsConfig,

    /// Per-package configuration for named packages in the package sets
    ///
    /// Which set doesn't matter, as a package can only be in one package set in
    /// the assembled image.
    pub package_configs: PackageConfigs,

    /// The list of domain configs to add.
    pub domain_configs: DomainConfigs,

    // The list of core shards to add.
    pub core_shards: Vec<Utf8PathBuf>,

    // The configuration capabilities to add (or route to "void").
    pub configuration_capabilities: CapabilityNamedMap,

    /// The Kernel Commandline Arguments to add.
    pub kernel_args: BTreeSet<String>,

    pub compiled_packages: BTreeMap<CompiledPackageDestination, CompiledPackageDefinition>,
}

/// A map from package names to the configuration to apply to them.
pub type PackageConfigs = NamedMap<String, PackageConfiguration>;

/// A map from component manifest path with a namespace to the values for the component.
pub type ComponentConfigs = NamedMap<String, ComponentConfiguration>;

/// A map from package name to domain config.
pub type DomainConfigs = NamedMap<PackageSetDestination, DomainConfig>;

/// All of the configuration that applies to a single package.
///
/// This holds:
/// - config_data entries for the package
/// - for each component:
///     - Structured Config
///
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PackageConfiguration {
    /// A map from manifest paths within the package namespace to the values for the component.
    pub components: ComponentConfigs,

    /// A map of config data entries, keyed by the destination path.
    pub config_data: NamedFileMap<String>,

    /// The package name.
    pub name: String,
}

impl PackageConfiguration {
    /// Construt a new PackageConfiguration.
    pub fn new(name: impl AsRef<str>) -> Self {
        PackageConfiguration {
            components: ComponentConfigs::new("component configs"),
            config_data: NamedFileMap::new("config data"),
            name: name.as_ref().into(),
        }
    }
}

/// All of the configuration for a single component.
///
/// This holds:
/// - Structured Config values for this component.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComponentConfiguration {
    /// Structured Config key-value pairs.
    pub fields: NamedMap<String, serde_json::Value>,

    /// The component's manifest path in its package or in bootfs
    manifest_path: String,
}

impl Default for ComponentConfiguration {
    fn default() -> Self {
        Self { fields: NamedMap::new("structured config fields"), manifest_path: String::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DomainConfig {
    pub directories: NamedMap<String, DomainConfigDirectory>,
    pub name: PackageSetDestination,
    pub expose_directories: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum FileOrContents {
    File(FileEntry<String>),
    Contents(String),
    BinaryContents(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DomainConfigDirectory {
    pub entries: NamedMap<String, FileOrContents>,
}

trait ICUMapExt<'a> {
    /// Resolves a [Revision] into a computed revision, and a commit ID. If both
    /// can not be computed, an error is returned.
    fn resolve_revision(&'a self, requested_revision: &'a Revision) -> Result<(&Revision, &str)>;
}

impl<'a> ICUMapExt<'a> for ICUMap {
    fn resolve_revision(&'a self, requested_revision: &'a Revision) -> Result<(&Revision, &str)> {
        let (computed_revision, computed_commit_id) = match requested_revision {
            Revision::Default | Revision::Latest => (
                Some(requested_revision),
                ICU_CONFIG_INFO.commit_id_for_revision(requested_revision),
            ),
            Revision::CommitId(commit_id) => {
                (self.revision_for_commit_id(commit_id), Some(commit_id.as_ref()))
            }
        };
        match (computed_revision, computed_commit_id) {
            (None, _) => Err(anyhow!("revision not found for: {:?}", requested_revision)),
            (_, None) => Err(anyhow!("commit ID not found for: {:?}", requested_revision)),
            (Some(revision), Some(commit_id)) => Ok((revision, commit_id)),
        }
    }
}

impl ConfigurationBuilder for ConfigurationBuilderImpl {
    fn add_auto_include_bundles(
        &mut self,
        platform_artifacts: &PlatformArtifacts,
        feature_set_level: &FeatureSetLevel,
        build_type: &BuildType,
    ) {
        let bundle_names = platform_artifacts.assembly_input_bundles.clone();
        for bundle_name in bundle_names {
            let aib_config_path = platform_artifacts.get_bundle(&bundle_name);
            let aib_dir = aib_config_path.parent().unwrap();
            let aib = AssemblyInputBundle::from_dir(aib_dir).unwrap();

            // Perform the "mutual exclusivity" check for the platform configuration.
            //
            // If the AIB has `auto_include_in` set (for *any* configuration), then it's
            // ineligible for being manually added by a subsystem.
            if !aib.auto_include_in.is_empty() {
                self.auto_includable_bundles.insert(bundle_name.clone());
            }

            if aib.should_be_auto_included_in(feature_set_level, build_type) {
                self.bundles.insert(bundle_name);
            }
        }
    }

    fn platform_bundle(&mut self, name: &str) -> Result<()> {
        if self.auto_includable_bundles.contains(name) {
            bail!(
                "The AIB '{}' has `auto_include_in` set, and so it cannot be manually included by a subsystem.",
                name
            );
        }
        self.bundles.insert(name.to_string());
        Ok(())
    }

    fn bootfs(&mut self) -> &mut dyn BootfsConfigBuilder {
        &mut self.bootfs
    }

    fn package(&mut self, name: &str) -> &mut dyn PackageConfigBuilder {
        self.package_configs.entry(name.to_string()).or_insert_with_key(|name| {
            PackageConfiguration {
                components: ComponentConfigs::new("component configs"),
                config_data: NamedFileMap::new("config data"),
                name: name.to_owned(),
            }
        })
    }

    fn add_domain_config(&mut self, name: PackageSetDestination) -> &mut dyn DomainConfigBuilder {
        self.domain_configs.entry(name).or_insert_with_key(|name| DomainConfig {
            directories: NamedMap::new("directories"),
            name: name.clone(),
            expose_directories: true,
        })
    }

    fn icu_platform_bundle(&mut self, name: &str) -> Result<()> {
        let icu_config = &self.icu_config;
        let bundle_name = {
            let (revision, commit_id) = ICU_CONFIG_INFO
                .resolve_revision(&icu_config.revision)
                .with_context(|| format!("while resolving revision: {}", icu_config.revision))?;
            format!("{}.icu_{}_{}", name, revision, commit_id)
        };
        self.platform_bundle(&bundle_name)?;
        Ok(())
    }

    fn core_shard(&mut self, path: &Utf8PathBuf) {
        self.core_shards.push(path.clone());
    }

    fn set_config_capability(
        &mut self,
        name: &str,
        config: assembly_config_capabilities::Config,
    ) -> Result<()> {
        self.configuration_capabilities.try_insert_unique(name.to_string(), config)
    }

    fn kernel_arg(&mut self, arg: KernelArg) {
        self.kernel_args.insert(arg.to_string());
    }

    fn compiled_package(
        &mut self,
        destination: CompiledPackageDestination,
        definition: CompiledPackageDefinition,
    ) -> Result<&mut CompiledPackageDefinition> {
        match self.compiled_packages.entry(destination) {
            Entry::Occupied(entry) => Err(anyhow!("duplicate package destination: {:?}", entry)),
            Entry::Vacant(entry) => Ok(entry.insert(definition)),
        }
    }
}

impl DomainConfigBuilder for DomainConfig {
    fn directory(&mut self, name: &str) -> &mut dyn DomainConfigDirectoryBuilder {
        self.directories
            .entry(name.to_string())
            .or_insert_with(|| DomainConfigDirectory { entries: NamedMap::new("domain configs") })
    }

    fn skip_expose(&mut self) -> &mut dyn DomainConfigBuilder {
        self.expose_directories = false;
        self
    }
}

impl DomainConfigDirectoryBuilder for DomainConfigDirectory {
    fn entry(
        &mut self,
        file_entry: FileEntry<String>,
    ) -> Result<&mut dyn DomainConfigDirectoryBuilder> {
        self.entries
            .try_insert_unique(file_entry.destination.clone(), FileOrContents::File(file_entry))
            .context("A config destination can only be set once for a domain config")?;
        Ok(self)
    }

    fn entry_from_contents(
        &mut self,
        destination: &str,
        contents: &str,
    ) -> Result<&mut dyn DomainConfigDirectoryBuilder> {
        self.entries
            .try_insert_unique(
                destination.to_string(),
                FileOrContents::Contents(contents.to_string()),
            )
            .context("A config destination can only be set once for a domain config")?;
        Ok(self)
    }

    fn entry_from_binary_contents(
        &mut self,
        destination: &str,
        contents: Vec<u8>,
    ) -> Result<&mut dyn DomainConfigDirectoryBuilder> {
        self.entries
            .try_insert_unique(destination.to_string(), FileOrContents::BinaryContents(contents))
            .context("A config destination can only be set once for a domain config")?;
        Ok(self)
    }
}

impl PackageConfigBuilder for PackageConfiguration {
    fn component(&mut self, pkg_path: &str) -> Result<&mut dyn ComponentConfigBuilder> {
        match self.components.entry(pkg_path.to_owned()) {
            entry @ Entry::Vacant(_) => {
                Ok(entry.or_insert_with_key(|path_in_package| ComponentConfiguration {
                    fields: NamedMap::new("structured config fields"),
                    manifest_path: path_in_package.to_owned(),
                }))
            }
            Entry::Occupied(_) => {
                Err(anyhow!("Each component's configuration can only be set once"))
                    .with_context(|| format!("Setting configuration for component: {pkg_path}"))
                    .with_context(|| anyhow!("Setting configuration for package: {}", self.name))
            }
        }
    }

    fn config_data(
        &mut self,
        file_entry: FileEntry<String>,
    ) -> Result<&mut dyn PackageConfigBuilder> {
        self.config_data
            .add_entry(file_entry)
            .context("A config data destination can only be set once for a package")?;
        Ok(self)
    }

    fn optional_config_data_files(
        &mut self,
        paths: Vec<(&Option<Utf8PathBuf>, &str)>,
    ) -> Result<&mut dyn PackageConfigBuilder> {
        for (source, destination) in paths {
            if let Some(source) = source {
                self.config_data(FileEntry {
                    source: source.clone(),
                    destination: destination.into(),
                })
                .with_context(|| format!("setting {destination}"))?;
            }
        }
        Ok(self)
    }
}

impl ComponentConfigBuilder for ComponentConfiguration {
    /// Add a value for a Structured Configuration field for a given component.
    fn field_value(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<&mut dyn ComponentConfigBuilder> {
        self.fields
            .try_insert_unique(key.to_owned(), value)
            .context("Each Structured Config field can only be set once for a component")?;
        Ok(self)
    }
}

/// Configuration of components in bootfs.
///
/// This is separate from PackageConfig because it may have to place bare files
/// in bootfs.
#[derive(Clone, Debug, PartialEq)]
pub struct BootfsConfig {
    /// A map from bootfs destination to bootfs file entry.
    pub files: NamedFileMap<BootfsDestination>,
}

impl Default for BootfsConfig {
    fn default() -> Self {
        Self { files: NamedFileMap::new("bootfs files") }
    }
}

impl BootfsConfigBuilder for BootfsConfig {
    /// Add a file to bootfs.
    fn file(
        &mut self,
        file_entry: FileEntry<BootfsDestination>,
    ) -> Result<&mut dyn BootfsConfigBuilder> {
        self.files.add_entry(file_entry)?;
        Ok(self)
    }
}

pub(crate) trait BoardConfigExt {
    /// Returns whether or not this board provides the named feature.
    fn provides_feature(&self, feature: BoardFeature) -> bool;
}

impl BoardConfigExt for BoardConfig {
    /// Returns whether or not this board provides the named feature.
    fn provides_feature(&self, feature: BoardFeature) -> bool {
        // .contains(&str) doesn't work for Vec<String>, so it's neccessary
        // to use .iter().any(...) instead.
        let feature_str = feature.as_ref();
        self.provided_features.iter().any(|f| f == feature_str)
    }
}

impl BoardConfigExt for Option<&BoardConfig> {
    fn provides_feature(&self, feature: BoardFeature) -> bool {
        match self {
            Some(board_config) => board_config.provides_feature(feature),
            _ => false,
        }
    }
}

/// DefaultByBuildType trait is implemented on each enum value to instantiate an
/// unconfigured update checker. Specifically, an Eng build-type will result in an
/// unconfigured system-update-checker and the User or UserDebug build-types will
/// result in an unconfigured omaha-client
///
/// ```
/// impl DefaultByBuildType for PolicyLabels {
///    fn default_by_build_type(build_type: &BuildType) -> Self {
///        match build_type {
///            BuildType::Eng => PolicyLabels::Unrestricted,
///            BuildType::UserDebug => PolicyLabels::LocalDynamicConfig,
///            BuildType::User => PolicyLabels::BaseComponentsOnly,
///        }
///    }
/// }
///
/// let policy = PolicyLabels::default_by_build_type(&BuildType::Eng);
/// assert_eq!(policy, PolicyLabels::Unrestricted);
/// ```
pub(crate) trait DefaultByBuildType {
    fn default_by_build_type(build_type: &BuildType) -> Self;
}

/// A trait which declares that a type T implements a fn which returns an instance of type T by
/// default (with respect to the build_type) or because the T struct has already been instantiated
pub(crate) trait OptionDefaultByBuildTypeExt<T: DefaultByBuildType> {
    #[allow(dead_code)]
    fn value_or_default_from_build_type(self, build_type: &BuildType) -> T;
}

/// Returns an unwrapped instance of T if T is provided, else defers to T::default_by_build_type
/// Used in situations where the configuration value's default is dependent on the build_type,
/// and may not be provided by the product owner. Therefore, None ends up being converted to
/// a default which depends on the provided BuildType.
///
/// Usage:
/// ```
/// let config = SwdConfig {
///    policy: None,
///    ...
/// };
/// let policy = config.policy.value_or_default_from_build_type(&BuildType::Eng);
/// assert_eq!(policy, PolicyLabels::Unrestricted);
/// ```
///
///
///
impl<T: DefaultByBuildType> OptionDefaultByBuildTypeExt<T> for Option<T> {
    fn value_or_default_from_build_type(self, build_type: &BuildType) -> T {
        self.unwrap_or_else(|| T::default_by_build_type(build_type))
    }
}
// An extension trait to get revision information out of an [ICUMap].
trait ICUMapGetExt {
    /// Gets the commit ID corresponding to the given `revision`.
    fn commit_id_for_revision(&self, revision: &Revision) -> Option<&str>;

    /// Maps back from `commit_id` to a `Revision` if possible.
    fn revision_for_commit_id(&self, commit_id: &str) -> Option<&Revision>;
}

impl ICUMapGetExt for ICUMap {
    fn commit_id_for_revision(&self, revision: &Revision) -> Option<&str> {
        self.0.get(revision).map(|k| k.as_str())
    }

    fn revision_for_commit_id(&self, commit_id: &str) -> Option<&Revision> {
        // Slow but easy inverse mapping, since the map is small.
        self.0.iter().filter(|(_, v)| &v[..] == commit_id).map(|(k, _)| k).next()
    }
}

impl ConfigurationContext<'_> {
    /// Return a new gendir that is nested under the top-level gendir
    /// Subsystems can use this to generate files before adding them to the
    /// builder.
    pub fn get_gendir(&self) -> Result<Utf8PathBuf> {
        std::fs::create_dir_all(&self.gendir)
            .map_err(|e| anyhow!("preparing new gendir: {}", e))?;
        Ok(self.gendir.to_owned())
    }

    /// Retrieve the full path to a platform resource identified by a path into
    /// the resource directory.
    pub fn get_resource(&self, path: impl AsRef<Utf8Path>) -> Utf8PathBuf {
        self.resource_dir.join(path.as_ref())
    }
}

/// This is the basis for the the context that's passed to each subsystem's configuration
/// module.  These are the fields from the
/// `assembly_config_schema::platform_settings::PlatformSettings` struct that are
/// available to all subsystems to use to derive their configuration from.  This is used to create
/// the specific ConfigurationContext object that's used for each individual subsystem.
#[derive(Debug)]
pub(crate) struct ConfigurationContextBase<'a> {
    pub base_context: ConfigurationContext<'a>,
}

impl<'a> ConfigurationContextBase<'a> {
    pub fn for_subsystem(&self, subsystem: &str) -> ConfigurationContext<'a> {
        ConfigurationContext {
            gendir: self.base_context.gendir.join(subsystem),
            resource_dir: self.base_context.resource_dir.to_owned(),
            ..self.base_context
        }
    }
}

#[cfg(test)]
impl ConfigurationContext<'_> {
    /// Use e.g. in tests that initialize only relevant fields.
    ///
    /// For example:
    ///
    /// ```
    ///  use crate::subsystems::prelude::*;
    ///  let context = ConfigurationContext {
    ///      feature_set_level: &FeatureSetLevel::Standard,
    ///      build_type: &BuildType::Eng,
    ///      ..Default::default()
    ///  };
    ///  ```
    pub(crate) fn default_for_tests() -> Self {
        Self {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::User,
            board_config: &tests::BOARD_INFORMATION_FOR_TESTS,
            gendir: Utf8PathBuf::from_path_buf(
                tempfile::TempDir::new().unwrap().path().to_path_buf(),
            )
            .unwrap(),
            resource_dir: Utf8PathBuf::new(),
            developer_only_options: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_images_config::BoardFilesystemConfig;
    use assembly_named_file_map::SourceMerklePair;
    use std::io::Write;
    use std::sync::LazyLock;
    use tempfile::TempDir;
    use utf8_path::path_relative_from_current_dir;

    pub(crate) static BOARD_INFORMATION_FOR_TESTS: LazyLock<BoardConfig> =
        LazyLock::new(|| BoardConfig {
            name: "Test Board".into(),
            provided_features: vec![],
            input_bundles: Default::default(),
            filesystems: BoardFilesystemConfig::default(),
            ..Default::default()
        });

    #[test]
    fn test_config_builder() {
        let mut builder = ConfigurationBuilderImpl::default();

        let temp_dir = TempDir::new().unwrap();
        let write_temp_file = |name: &str, contents: &str| -> Utf8PathBuf {
            let p = Utf8PathBuf::from_path_buf(temp_dir.path().join(name)).unwrap();
            let mut f = std::fs::File::create(&p).unwrap();
            write!(&mut f, "{contents}").unwrap();
            p
        };
        let one = write_temp_file("config_data1", "one");
        let two = write_temp_file("config_data2", "two");
        let relative_one = path_relative_from_current_dir(&one).unwrap();
        let relative_two = path_relative_from_current_dir(&two).unwrap();

        // using an inner
        let make_config = |builder: &mut dyn ConfigurationBuilder| -> Result<()> {
            builder
                .package("package_a")
                .component("meta/component_a1")?
                .field("key_a1", "value_a1")?;
            builder
                .package("package_a")
                .component("meta/component_a2")?
                .field("key_a2", "value_a2")?;
            builder
                .package("package_b")
                .component("meta/component_b")?
                .field("key_b1", "value_b1")?
                .field("key_b2", "value_b2")?;

            builder
                .package("package_a")
                .config_data(FileEntry { destination: "config/one".into(), source: one.clone() })?;
            builder
                .package("package_a")
                .config_data(FileEntry { destination: "config/two".into(), source: two.clone() })?;
            builder
                .package("package_b")
                .config_data(FileEntry { destination: "config/one".into(), source: one.clone() })?;

            Ok(())
        };
        assert!(make_config(&mut builder).is_ok());
        let config = builder.build();

        assert_eq!(config.package_configs.len(), 2);
        assert_eq!(
            config.package_configs.get("package_a").unwrap(),
            &PackageConfiguration {
                name: "package_a".into(),
                components: NamedMap {
                    name: "component configs".into(),
                    entries: [
                        (
                            "meta/component_a1".into(),
                            ComponentConfiguration {
                                manifest_path: "meta/component_a1".into(),
                                fields: NamedMap {
                                    name: "structured config fields".into(),
                                    entries: [("key_a1".into(), "value_a1".into())].into()
                                },
                            }
                        ),
                        (
                            "meta/component_a2".into(),
                            ComponentConfiguration {
                                manifest_path: "meta/component_a2".into(),
                                fields: NamedMap {
                                    name: "structured config fields".into(),
                                    entries: [("key_a2".into(), "value_a2".into())].into(),
                                },
                            }
                        ),
                    ]
                    .into(),
                },
                config_data: NamedFileMap {
                    map: NamedMap {
                        name: "config data".into(),
                        entries: [
                            (
                                "config/one".into(),
                                SourceMerklePair { merkle: None, source: relative_one.clone() }
                            ),
                            (
                                "config/two".into(),
                                SourceMerklePair { merkle: None, source: relative_two.clone() }
                            ),
                        ]
                        .into(),
                    },
                },
            }
        );
        assert_eq!(
            config.package_configs.get("package_b").unwrap(),
            &PackageConfiguration {
                name: "package_b".into(),
                components: NamedMap {
                    name: "component configs".into(),
                    entries: [(
                        "meta/component_b".into(),
                        ComponentConfiguration {
                            manifest_path: "meta/component_b".into(),
                            fields: NamedMap {
                                name: "structured config fields".into(),
                                entries: [
                                    ("key_b1".into(), "value_b1".into()),
                                    ("key_b2".into(), "value_b2".into())
                                ]
                                .into(),
                            },
                        }
                    )]
                    .into(),
                },
                config_data: NamedFileMap {
                    map: NamedMap {
                        name: "config data".into(),
                        entries: [(
                            "config/one".into(),
                            SourceMerklePair { merkle: None, source: relative_one.clone() }
                        )]
                        .into(),
                    },
                },
            }
        );
    }

    #[test]
    fn test_multiple_adds_fail() {
        let temp_dir = TempDir::new().unwrap();
        let mut builder = ConfigurationBuilderImpl::default();

        assert!(builder.package("foo").component("bar").is_ok());
        assert!(builder.package("foo").component("bar").is_err());

        let mut component = builder.package("foo").component("baz").unwrap();
        assert!(component.field("key", "value").is_ok());
        assert!(component.field("key", "diff_value").is_err());
        assert!(component.field("key2", "value2").is_ok());
        assert!(component.field("key2", "value2").is_err());

        let write_temp_file = |name: &str, contents: &str| -> Utf8PathBuf {
            let p = Utf8PathBuf::from_path_buf(temp_dir.path().join(name)).unwrap();
            let mut f = std::fs::File::create(&p).unwrap();
            write!(&mut f, "{contents}").unwrap();
            p
        };
        let baz = write_temp_file("baz", "baz");
        let cat = write_temp_file("cat", "cat");
        let diz = write_temp_file("diz", "diz");

        assert!(
            builder
                .package("foo")
                .config_data(FileEntry { destination: "bar".into(), source: baz })
                .is_ok()
        );
        assert!(
            builder
                .package("foo")
                .config_data(FileEntry { destination: "cat".into(), source: cat.clone() })
                .is_ok()
        );
        assert!(
            builder
                .package("foo")
                .config_data(FileEntry { destination: "cat".into(), source: diz })
                .is_err()
        );
    }

    #[test]
    fn test_error_messages_for_multiple_adds() {
        // This test validates that the error messages produced by the builder,
        // when their context() entries are flattened, produces sensical-looking
        // errors.

        fn format_result<T>(result: Result<T>) -> String {
            if let Err(e) = result {
                format!(
                    "{} Failed{}",
                    e,
                    e.chain()
                        .skip(1)
                        .enumerate()
                        .map(|(i, e)| format!("\n  {: >3}.  {}", i + 1, e))
                        .collect::<Vec<String>>()
                        .concat()
                )
            } else {
                "Not An error".into()
            }
        }

        let mut builder = ConfigurationBuilderImpl::default();

        builder.package("foo").component("bar").unwrap();
        let result = builder.package("foo").component("bar").context("Configuring Subsystem");

        assert_eq!(
            format_result(result),
            r"Configuring Subsystem Failed
    1.  Setting configuration for package: foo
    2.  Setting configuration for component: bar
    3.  Each component's configuration can only be set once"
        );

        let mut component = builder.package("other").component("bar").unwrap();
        component.field("key", "value").unwrap();
        let result = component.field("key", "value2").context("Configuring Subsystem");

        assert_eq!(
            format_result(result),
            r#"Configuring Subsystem Failed
    1.  Each Structured Config field can only be set once for a component
    2.  duplicate entry in structured config fields:
  key: 'key'
  existing value: String("value")
  new value: String("value2")"#
        );
    }

    #[test]
    fn test_provides_feature() {
        let board_config = BoardConfig {
            name: "sample".to_owned(),
            provided_features: vec!["fuchsia::fan".into(), "fuchsia::gnss".into()],
            ..Default::default()
        };

        assert!(board_config.provides_feature(BoardFeature::Fan));
        assert!(board_config.provides_feature(BoardFeature::Gnss));
        assert!(!board_config.provides_feature(BoardFeature::Input));
    }

    #[test]
    fn check_icu_map() {
        assert!(ICU_CONFIG_INFO.commit_id_for_revision(&Revision::Latest).is_some());
        assert!(ICU_CONFIG_INFO.commit_id_for_revision(&Revision::Default).is_some());
    }

    #[test]
    fn test_ensure_build_type_and_feature_set() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Bootstrap,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                name: "sample".to_owned(),
                provided_features: vec!["feature_a".into(), "feature_b".into()],
                ..Default::default()
            },
            gendir: "gendir".into(),
            resource_dir: "resources".into(),
            developer_only_options: Default::default(),
        };
        let result = context.ensure_build_type_and_feature_set_level(
            &[BuildType::Eng],
            &[FeatureSetLevel::Bootstrap],
            "feature",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_build_type_and_feature_set_bails_on_bad_build_type() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Bootstrap,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                name: "sample".to_owned(),
                provided_features: vec!["feature_a".into(), "feature_b".into()],
                ..Default::default()
            },
            gendir: "gendir".into(),
            resource_dir: "resources".into(),
            developer_only_options: Default::default(),
        };
        let result = context.ensure_build_type_and_feature_set_level(
            &[BuildType::User, BuildType::UserDebug],
            &[FeatureSetLevel::Bootstrap],
            "My feature",
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "My feature can only be enabled on the following build types, not 'eng': [user, userdebug]"
        )
    }

    #[test]
    fn test_ensure_build_type_and_feature_set_bails_on_bad_feature_set_level() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Bootstrap,
            build_type: &BuildType::Eng,
            board_config: &BoardConfig {
                name: "sample".to_owned(),
                provided_features: vec!["feature_a".into(), "feature_b".into()],
                ..Default::default()
            },
            gendir: "gendir".into(),
            resource_dir: "resources".into(),
            developer_only_options: Default::default(),
        };
        let result = context.ensure_build_type_and_feature_set_level(
            &[BuildType::Eng],
            &[FeatureSetLevel::Utility, FeatureSetLevel::Standard],
            "My feature",
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "My feature can only be enabled on the following feature sets, not 'bootstrap': [utility, standard]"
        )
    }

    #[test]
    fn test_error_on_manual_include_of_auto_included() {
        let mut builder = ConfigurationBuilderImpl::default();
        builder.auto_includable_bundles.insert("foo".to_string());
        assert!(builder.platform_bundle("foo").is_err());
    }
}
