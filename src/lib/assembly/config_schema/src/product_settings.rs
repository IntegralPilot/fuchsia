// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;

use anyhow::Result;
use assembly_container::{FileType, WalkPaths, WalkPathsFn};
use assembly_package_utils::{PackageInternalPathBuf, PackageManifestPathBuf};
use camino::{Utf8Path, Utf8PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{DriverDetails, path_schema, vec_path_schema};
use assembly_release_info::ProductReleaseInfo;

/// The Product-provided configuration details.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, WalkPaths, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProductSettings {
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub packages: ProductPackagesConfig,

    /// List of base drivers to include in the product.
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub base_drivers: Vec<DriverDetails>,

    /// Product-specific session information.
    ///
    /// Default to None which creates a "paused" config that launches nothing to start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<ProductSessionConfig>,

    /// Generic product information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<ProductInfoConfig>,

    /// The file paths to various build information.
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_info: Option<BuildInfoConfig>,

    /// The policy given to component_manager that restricts where sensitive capabilities can be
    /// routed.
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub component_policy: ComponentPolicyConfig,

    /// Configuration strategy for TEE.
    #[walk_paths]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub tee: Tee,

    /// Components which depend on trusted applications running in the TEE.
    ///
    /// *NOTE*: This configuration parameter is deprecated. Use
    /// `tee = Tee::GlobalPlatform(tee_clients)` instead.
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tee_clients: Vec<GlobalPlatformTeeClient>,

    /// Components which should run as trusted applications in Fuchsia.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trusted_apps: Vec<TrustedApp>,

    /// A package that includes files to include in bootfs.
    ///
    /// This is only usable in the empty, embeddable, and bootstrap feature set levels.
    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootfs_files_package: OptionalPackage,

    /// Release information about this assembly container artifact.
    pub release_info: ProductReleaseInfo,

    /// Inputs for generating Starnix Containers
    #[walk_paths]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub starnix_containers: Vec<StarnixContainerConfig>,
}

/// Packages provided by the product, to add to the assembled images.
///
/// This also includes configuration for those packages:
///
/// ```json5
///   packages: {
///     base: {
///       package_a: {
///         manifest: "path/to/package_a/package_manifest.json",
///       },
///       package_b: {
///         manifest: "path/to/package_b/package_manifest.json",
///         config_data: {
///           "foo.cfg": "path/to/some/source/file/foo.cfg",
///           "bar/more/data.json": "path/to/some.json",
///         },
///       },
///     ],
///     cache: []
///   }
/// ```
///
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields, from = "ProductPackagesConfigDeserializeHelper")]
pub struct ProductPackagesConfig {
    /// Paths to package manifests, or more detailed json entries for packages
    /// to add to the 'base' package set, which are keyed by package name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub base: BTreeMap<String, ProductPackageDetails>,

    /// Paths to package manifests, or more detailed json entries for packages
    /// to add to the 'cache' package set, which are keyed by package name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cache: BTreeMap<String, ProductPackageDetails>,

    /// Paths to package manifests, or more detailed json entries for packages
    /// to add to the 'bootfs' package set, which are keyed by package name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bootfs: BTreeMap<String, ProductPackageDetails>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProductPackagesConfigDeserializeHelper {
    pub base: MapOrVecOfPackages,
    pub cache: MapOrVecOfPackages,
    pub bootfs: MapOrVecOfPackages,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct StarnixContainerConfig {
    /// Name of the starnix container
    pub name: String,
    /// Path to package containing base resources to include.
    pub base: String,
    /// Config for HAL packages
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hals: Vec<String>,
    /// Whether to skip including HALs as subpackages.
    pub skip_subpackages: bool,
    /// Path to fstab, will go in /odm which overrides the one in /vendor
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fstab: Option<Utf8PathBuf>,
    /// Path to extra init scripts, will go in /odm/etc/init. Can be passed more than once.
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub init: Vec<Utf8PathBuf>,
    /// File overrides to apply to the container.
    #[walk_paths]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_overrides: Vec<StarnixFileOverride>,

    /// Initially, we have images, but at some point we have converted it
    /// to a package, and we should track that for hybrid assembly.
    ///
    /// The fields above are whatever we need for either container generation
    /// or hybrid container generation. If there is something we only need
    /// for container generation, we could consider moving it into StarnixImages.
    #[walk_paths]
    #[serde(skip_serializing_if = "StarnixImagesOrPackage::is_images")]
    pub images_or_package: StarnixImagesOrPackage,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq, Clone)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum StarnixImagesOrPackage {
    Images(StarnixImages),
    #[schemars(schema_with = "crate::common::path_schema")]
    Package(Utf8PathBuf),
}

impl Default for StarnixImagesOrPackage {
    fn default() -> Self {
        Self::Images(StarnixImages::default())
    }
}

impl WalkPaths for StarnixImagesOrPackage {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> anyhow::Result<()> {
        match self {
            Self::Images(images) => images.walk_paths_with_dest(found, dest.join("images")),
            Self::Package(manifest) => {
                found(manifest, dest.join("package"), FileType::PackageManifest)
            }
        }
    }
}

impl StarnixImagesOrPackage {
    pub fn is_images(&self) -> bool {
        matches!(self, Self::Images(_))
    }
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Clone, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct StarnixImages {
    /// Path to an Android system image
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    pub system: Utf8PathBuf,
    /// Path to an Android vendor partition image
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<Utf8PathBuf>,
    /// Path to the ramdisk images
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ramdisk: Vec<Utf8PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq, WalkPaths, Clone)]
pub struct StarnixFileOverride {
    /// Name of the image to override files in (e.g. system, vendor, odm)
    pub image_name: String,
    /// Path to the file in the image (e.g. system/bin/sh)
    pub file_path: String,
    /// Operation to perform
    pub operation: StarnixFileOperation,
    /// Mode for the file (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u16>,
    /// UID for the file (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u16>,
    /// GID for the file (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u16>,

    /// Security label (SELinux context) for the file (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seclabel: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum StarnixFileOperation {
    Overwrite(#[schemars(schema_with = "path_schema")] Utf8PathBuf),
    Create(#[schemars(schema_with = "path_schema")] Utf8PathBuf),
    Remove,
}

impl WalkPaths for StarnixFileOperation {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> anyhow::Result<()> {
        match self {
            Self::Overwrite(path) | Self::Create(path) => found(path, dest, FileType::Unknown),
            Self::Remove => Ok(()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MapOrVecOfPackages {
    Map(BTreeMap<String, ProductPackageDetails>),
    Vec(Vec<ProductPackageDetails>),
}

impl Default for MapOrVecOfPackages {
    fn default() -> Self {
        Self::Map(BTreeMap::default())
    }
}

fn convert_to_map(map_or_vec: MapOrVecOfPackages) -> BTreeMap<String, ProductPackageDetails> {
    match map_or_vec {
        MapOrVecOfPackages::Map(map) => map,
        // The key in the map defaults to the index in the vector.
        MapOrVecOfPackages::Vec(vec) => {
            vec.into_iter().enumerate().map(|(i, s)| (i.to_string(), s)).collect()
        }
    }
}

impl From<ProductPackagesConfigDeserializeHelper> for ProductPackagesConfig {
    fn from(helper: ProductPackagesConfigDeserializeHelper) -> Self {
        Self {
            base: convert_to_map(helper.base),
            cache: convert_to_map(helper.cache),
            bootfs: convert_to_map(helper.bootfs),
        }
    }
}

/// Describes in more detail a package to add to the assembly.
#[derive(Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductPackageDetails {
    /// Path to the package manifest for this package.
    #[schemars(schema_with = "path_schema")]
    pub manifest: Utf8PathBuf,

    /// Map of config_data entries for this package, from the destination path
    /// within the package, to the path where the source file is to be found.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub config_data: Vec<ProductConfigData>,
}

fn walk_package_set<F>(
    set: &mut BTreeMap<String, ProductPackageDetails>,
    found: &mut F,
    dest: Utf8PathBuf,
) -> Result<()>
where
    F: WalkPathsFn,
{
    for (name, pkg) in set {
        let pkg_dest = dest.join(name);
        found(&mut pkg.manifest, pkg_dest.clone(), FileType::PackageManifest)?;

        // Add the config data so that it is identified by package name and destination.
        // This ensures that we do not have collisions between inputs with the same name.
        // For example: `{pkg_dest}/config_data/config.txt`
        for config in &mut pkg.config_data {
            let config_dest = pkg_dest.join("config_data").join(&config.destination);
            found(&mut config.source, config_dest, FileType::Unknown)?;
        }
    }
    Ok(())
}

impl WalkPaths for ProductPackagesConfig {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> anyhow::Result<()> {
        walk_package_set(&mut self.base, found, dest.join("base"))?;
        walk_package_set(&mut self.cache, found, dest.join("cache"))?;
        walk_package_set(&mut self.bootfs, found, dest.join("bootfs"))?;
        Ok(())
    }
}

/// A newtype wrapper for the path to an optional package manifest.
#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptionalPackage(Option<Utf8PathBuf>);

impl std::ops::Deref for OptionalPackage {
    type Target = Option<Utf8PathBuf>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Option<Utf8PathBuf>> for OptionalPackage {
    fn as_ref(&self) -> &Option<Utf8PathBuf> {
        &self.0
    }
}

impl WalkPaths for OptionalPackage {
    fn walk_paths_with_dest<F: WalkPathsFn>(
        &mut self,
        found: &mut F,
        dest: Utf8PathBuf,
    ) -> anyhow::Result<()> {
        if let Some(path) = &mut self.0 {
            found(path, dest, FileType::PackageManifest)?;
        }
        Ok(())
    }
}

impl From<PackageManifestPathBuf> for ProductPackageDetails {
    fn from(manifest: PackageManifestPathBuf) -> Self {
        let manifestpath: &Utf8Path = manifest.as_ref();
        let path: Utf8PathBuf = manifestpath.into();
        Self { manifest: path, config_data: Vec::default() }
    }
}

impl From<&str> for ProductPackageDetails {
    fn from(s: &str) -> Self {
        ProductPackageDetails { manifest: s.into(), config_data: Vec::default() }
    }
}

#[derive(Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductConfigData {
    /// Path to the config file on the host.
    #[schemars(schema_with = "path_schema")]
    pub source: Utf8PathBuf,

    /// Path to find the file in the package on the target.
    pub destination: PackageInternalPathBuf,
}

/// Configuration options for product info.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductInfoConfig {
    /// Name of the product.
    pub name: String,
    /// Model of the product.
    pub model: String,
    /// Manufacturer of the product.
    pub manufacturer: String,
}

/// Configuration options for build info.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct BuildInfoConfig {
    /// Name of the product build target.
    pub name: String,
    /// Path to the version file.
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    pub version: Utf8PathBuf,
    /// Path to the jiri snapshot.
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    pub jiri_snapshot: Utf8PathBuf,
    /// Path to the latest commit date.
    #[walk_paths]
    #[schemars(schema_with = "path_schema")]
    pub latest_commit_date: Utf8PathBuf,
}

/// Configuration options for the component policy.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct ComponentPolicyConfig {
    /// The file paths to a product-provided component policies.
    #[walk_paths]
    #[schemars(schema_with = "vec_path_schema")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub product_policies: Vec<Utf8PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct TeeClientFeatures {
    /// Whether this component needs /dev-class/securemem routed to it. If true, the securemem
    //// directory will be routed as dev-securemem.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub securemem: bool,
    /// Whether this component requires persistent storage, routed as /data.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub persistent_storage: bool,
    /// Whether this component requires tmp storage, routed as /tmp.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub tmp_storage: bool,
}

/// Configuration strategy for TEE.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, WalkPaths, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Tee {
    /// Transitional value for deprecating `ProductSettings::tee_clients`. When this value is used,
    /// the `Tee::GlobalPlatform` strategy is deployed using `ProductSettings::tee_clients`.
    #[default]
    Undefined,
    /// Do not instantiate a TEE manager and associated routes.
    NoTee,
    /// Instantiate the platform-provided Global Platform (OPTEE) `tee_manager` component, with the
    /// provided `GlobalPlatformTeeClient` specifications.
    GlobalPlatform(GlobalPlatformTee),
}

/// Configuration of platform-provided TEE stack.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, WalkPaths, PartialEq)]
pub struct GlobalPlatformTee {
    /// Components which depend on trusted applications running in the TEE.
    ///
    /// *NOTE*: This configuration parameter replaces `ProductSettings::tee_clients`.
    pub clients: Vec<GlobalPlatformTeeClient>,
}

/// A configuration for a component which depends on TEE-based protocols.
/// Examples include components which implement DRM, or authentication services.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, WalkPaths, PartialEq)]
pub struct GlobalPlatformTeeClient {
    /// The URL of the component.
    pub component_url: String,
    /// GUIDs which of the form fuchsia.tee.Application.{GUID} will match a
    /// protocol provided by the TEE.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub guids: Vec<String>,
    /// Capabilities provided by this component which should be routed to the
    /// rest of the system.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Additional protocols which are required for this component to work, and
    /// which will be routed from 'parent'
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub additional_required_protocols: Vec<String>,
    /// Config data files required for this component to work, and which will be inserted into
    /// config data for this package (with a package name based on the component URL)
    #[serde(default)]
    #[walk_paths]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_data: Option<TeeClientConfigData>,
    /// Additional features required for the component to function.
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub additional_required_features: TeeClientFeatures,
}

/// The map of config data files for the TeeClient.
///
/// We have to wrap the BTreeMap in order to be able to implement JsonSchema
/// for a Utf8PathBuf.
#[derive(Clone, Debug, Deserialize, Serialize, WalkPaths, PartialEq)]
pub struct TeeClientConfigData {
    /// The inner map of config data files.
    #[serde(flatten)]
    #[walk_paths]
    pub files: BTreeMap<String, Utf8PathBuf>,
}

impl JsonSchema for TeeClientConfigData {
    fn schema_name() -> String {
        "TeeClientConfigData".into()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema: schemars::schema::SchemaObject =
            <BTreeMap<String, String>>::json_schema(generator).into();
        schema.format = Some("BTreeMap<String, Utf8PathBuf>".to_owned());
        schema.into()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
pub enum TrustedAppType {
    GlobalPlatform,
    BinderRPC,
}

/// Configuration for how to run a trusted application in Fuchsia.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct TrustedApp {
    /// The URL of the component.
    pub component_url: String,
    /// The GUID that identifies this trusted app for clients.
    pub guid: String,
    /// Type of trusted application.
    pub ta_type: TrustedAppType,
}

/// Product configuration options for the session:
///
/// ```json5
///   session: {
///     url: "fuchsia-pkg://fuchsia.com/my_session#meta/my_session.cm",
///     initial_element: {
///         collection: "elements",
///         url: "fuchsia-pkg://fuchsia.com/my_component#meta/my_component.cm"
///         view_id_annotation: "my_component"
///     }
///   }
/// ```
///
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductSessionConfig {
    /// Start URL to pass to `session_manager`.
    pub url: String,

    /// Specifies initial element properties for the window manager.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_element: Option<InitialElement>,
}

/// Platform configuration options for the window manager.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitialElement {
    /// Specifies the collection in which the window manager should launch the
    /// initial element, if one is given by `url`. Defaults to "elements".
    #[serde(default = "collection_default")]
    pub collection: String,

    /// Specifies the Fuchsia package URL of the element the window manager
    /// should launch on startup, if one is given.
    pub url: String,

    /// Specifies the annotation value by which the window manager can identify
    /// a view presented by the element it launched on startup, if one is given
    /// by `url`.
    pub view_id_annotation: String,
}

fn collection_default() -> String {
    String::from("elements")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_util as util;

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<ProductSettings>();
    }

    #[test]
    fn test_product_provided_config_data() {
        let json5 = r#"
            {
                base: [
                    {
                        manifest: "path/to/base/package_manifest.json"
                    },
                    {
                        manifest: "some/other/manifest.json",
                        config_data: [
                            {
                                destination: "dest/path/cfg.txt",
                                source: "source/path/cfg.txt",
                            },
                            {
                                destination: "other_data.json",
                                source: "source_other_data.json",
                            },
                        ]
                    }
                ],
                cache: [
                    {
                        manifest: "path/to/cache/package_manifest.json"
                    }
                ],
                bootfs: [
                    {
                        manifest: "path/to/bootfs/package_manifest.json"
                    }
                ]
            }
        "#;

        let mut cursor = std::io::Cursor::new(json5);
        let packages: ProductPackagesConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(
            packages.base,
            [
                (
                    "0".to_string(),
                    ProductPackageDetails {
                        manifest: "path/to/base/package_manifest.json".into(),
                        config_data: Vec::default()
                    }
                ),
                (
                    "1".to_string(),
                    ProductPackageDetails {
                        manifest: "some/other/manifest.json".into(),
                        config_data: vec![
                            ProductConfigData {
                                destination: "dest/path/cfg.txt".into(),
                                source: "source/path/cfg.txt".into(),
                            },
                            ProductConfigData {
                                destination: "other_data.json".into(),
                                source: "source_other_data.json".into(),
                            },
                        ]
                    }
                ),
            ]
            .into()
        );
        assert_eq!(
            packages.cache,
            [(
                "0".to_string(),
                ProductPackageDetails {
                    manifest: "path/to/cache/package_manifest.json".into(),
                    config_data: Vec::default()
                }
            )]
            .into()
        );
        assert_eq!(
            packages.bootfs,
            [(
                "0".to_string(),
                ProductPackageDetails {
                    manifest: "path/to/bootfs/package_manifest.json".into(),
                    config_data: Vec::default()
                }
            )]
            .into()
        );
    }

    #[test]
    fn product_packages_config_deserialization() {
        let from_list = r#"
            {
                base: [{
                    manifest: "some/other/manifest.json",
                    config_data: [
                        {
                            destination: "dest/path/cfg.txt",
                            source: "source/path/cfg.txt",
                        },
                        {
                            destination: "other_data.json",
                            source: "source_other_data.json",
                        },
                    ]
                }]
            }
        "#;
        let from_map = r#"
            {
                base: {
                    "0": {
                        manifest: "some/other/manifest.json",
                        config_data: [
                            {
                                destination: "dest/path/cfg.txt",
                                source: "source/path/cfg.txt",
                            },
                            {
                                destination: "other_data.json",
                                source: "source_other_data.json",
                            },
                        ]
                    }
                }
            }
        "#;
        let expected = ProductPackagesConfig {
            base: [(
                "0".to_string(),
                ProductPackageDetails {
                    manifest: "some/other/manifest.json".into(),
                    config_data: vec![
                        ProductConfigData {
                            destination: "dest/path/cfg.txt".into(),
                            source: "source/path/cfg.txt".into(),
                        },
                        ProductConfigData {
                            destination: "other_data.json".into(),
                            source: "source_other_data.json".into(),
                        },
                    ],
                },
            )]
            .into(),
            cache: BTreeMap::default(),
            bootfs: BTreeMap::default(),
        };

        let mut cursor = std::io::Cursor::new(from_list);
        let details: ProductPackagesConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(details, expected);

        let mut cursor = std::io::Cursor::new(from_map);
        let details: ProductPackagesConfig = util::from_reader(&mut cursor).unwrap();
        assert_eq!(details, expected);
    }

    #[test]
    fn product_package_details_deserialization() {
        let json5 = r#"
            {
                manifest: "some/other/manifest.json",
                config_data: [
                    {
                        destination: "dest/path/cfg.txt",
                        source: "source/path/cfg.txt",
                    },
                    {
                        destination: "other_data.json",
                        source: "source_other_data.json",
                    },
                ]
            }
        "#;
        let expected = ProductPackageDetails {
            manifest: "some/other/manifest.json".into(),
            config_data: vec![
                ProductConfigData {
                    destination: "dest/path/cfg.txt".into(),
                    source: "source/path/cfg.txt".into(),
                },
                ProductConfigData {
                    destination: "other_data.json".into(),
                    source: "source_other_data.json".into(),
                },
            ],
        };
        let mut cursor = std::io::Cursor::new(json5);
        let details: ProductPackageDetails = util::from_reader(&mut cursor).unwrap();
        assert_eq!(details, expected);
    }

    #[test]
    fn product_package_details_serialization() {
        let entries = vec![
            ProductPackageDetails {
                manifest: "path/to/manifest.json".into(),
                config_data: Vec::default(),
            },
            ProductPackageDetails {
                manifest: "another/path/to/a/manifest.json".into(),
                config_data: vec![
                    ProductConfigData {
                        destination: "dest/path/A".into(),
                        source: "source/path/A".into(),
                    },
                    ProductConfigData {
                        destination: "dest/path/B".into(),
                        source: "source/path/B".into(),
                    },
                ],
            },
        ];
        let serialized = serde_json::to_value(entries).unwrap();
        let expected = serde_json::json!(
            [
                {
                    "manifest": "path/to/manifest.json"
                },
                {
                    "manifest": "another/path/to/a/manifest.json",
                    "config_data": [
                        {
                            "destination": "dest/path/A",
                            "source": "source/path/A",
                        },
                        {
                            "destination": "dest/path/B",
                            "source": "source/path/B",
                        },
                    ]
                }
            ]
        );
        assert_eq!(serialized, expected);
    }

    #[test]
    fn product_tee_serialization() {
        let product_config = ProductSettings { tee: Tee::Undefined, ..Default::default() };
        let serialized = serde_json::to_value(product_config).unwrap();
        let expected = serde_json::json!({"release_info": ProductReleaseInfo::new_for_testing()});
        assert_eq!(serialized, expected);

        let product_config = ProductSettings { tee: Tee::NoTee, ..Default::default() };
        let serialized = serde_json::to_value(product_config).unwrap();
        let expected = serde_json::json!(
            {
                "tee": "no_tee",
                "release_info": ProductReleaseInfo::new_for_testing(),
            }
        );
        assert_eq!(serialized, expected);

        let product_config = ProductSettings {
            tee: Tee::GlobalPlatform(GlobalPlatformTee { clients: vec![] }),
            ..Default::default()
        };
        let serialized = serde_json::to_value(product_config).unwrap();
        let expected = serde_json::json!(
            {
                "tee": {
                    "global_platform": {
                        "clients": []
                    }
                },
                "release_info": ProductReleaseInfo::new_for_testing(),
            }
        );
        assert_eq!(serialized, expected);
    }

    #[test]
    fn product_tee_deserialization() {
        let json5 = r#"{}"#;
        let expected = ProductSettings { tee: Tee::Undefined, ..Default::default() };
        let mut cursor = std::io::Cursor::new(json5);
        let product_config: ProductSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(product_config, expected);

        let json5 = r#"{
            tee: "no_tee",
            release_info: {
                info: {
                    name: "",
                    repository: "",
                    version: ""
                },
                pibs: []
            }
        }"#;
        let expected = ProductSettings { tee: Tee::NoTee, ..Default::default() };
        let mut cursor = std::io::Cursor::new(json5);
        let product_config: ProductSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(product_config, expected);

        let json5 = r#"{
            tee: {
                global_platform: {
                    clients: [],
                },
            },
            release_info: {
                info: {
                    name: "",
                    repository: "",
                    version: ""
                },
                pibs: []
            }
        }"#;
        let expected = ProductSettings {
            tee: Tee::GlobalPlatform(GlobalPlatformTee { clients: vec![] }),
            ..Default::default()
        };
        let mut cursor = std::io::Cursor::new(json5);
        let product_config: ProductSettings = util::from_reader(&mut cursor).unwrap();
        assert_eq!(product_config, expected);
    }

    #[test]
    fn test_starnix_images_or_package_walk_paths() {
        // Test `Images` variant.
        let mut images_or_package = StarnixImagesOrPackage::Images(StarnixImages {
            system: "path/to/system".into(),
            vendor: Some("path/to/vendor".into()),
            ramdisk: vec!["path/to/ramdisk".into()],
        });
        let mut paths: Vec<(Utf8PathBuf, Utf8PathBuf, FileType)> = Vec::new();
        let mut callback = |path: &mut Utf8PathBuf, dest: Utf8PathBuf, file_type: FileType| {
            paths.push((path.clone(), dest, file_type));
            Ok(())
        };
        images_or_package.walk_paths_with_dest(&mut callback, "dest".into()).unwrap();

        let expected_images: Vec<(Utf8PathBuf, Utf8PathBuf, FileType)> = vec![
            ("path/to/system".into(), "dest/images/system".into(), FileType::Unknown),
            ("path/to/vendor".into(), "dest/images/vendor".into(), FileType::Unknown),
            ("path/to/ramdisk".into(), "dest/images/ramdisk/0".into(), FileType::Unknown),
        ];
        assert_eq!(paths, expected_images);

        // Test `Package` variant.
        let mut images_or_package = StarnixImagesOrPackage::Package("path/to/package".into());
        let mut paths: Vec<(Utf8PathBuf, Utf8PathBuf, FileType)> = Vec::new();
        let mut callback = |path: &mut Utf8PathBuf, dest: Utf8PathBuf, file_type: FileType| {
            paths.push((path.clone(), dest, file_type));
            Ok(())
        };
        images_or_package.walk_paths_with_dest(&mut callback, "dest".into()).unwrap();

        let expected_package: Vec<(Utf8PathBuf, Utf8PathBuf, FileType)> =
            vec![("path/to/package".into(), "dest/package".into(), FileType::PackageManifest)];
        assert_eq!(paths, expected_package);
    }
}
