// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::package_reader::{
    PackageReader, PackagesFromUpdateReader, read_partial_package_definition,
};
use crate::package_types::{ComponentManifest, PackageDefinition};
use crate::package_utils::is_cf_v2_manifest;
use anyhow::{Context, Result, anyhow};
use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use cm_fidl_analyzer::{PkgUrlMatch, match_absolute_pkg_urls};
use directed_graph::DirectedGraph;
use fidl::unpersist;
use fidl_fuchsia_component_decl as fdecl;
use fuchsia_merkle::Hash;
use fuchsia_url::fuchsia_pkg::{AbsoluteComponentUrl, AbsolutePackageUrl};
use fuchsia_url::{PackageName, PackageVariant};
use log::{info, warn};
use scrutiny_collection::core::{
    Component, ComponentSource, Components, CoreDataDeps, Manifest, ManifestData, Manifests,
    Package, Packages,
};
use scrutiny_collection::model::DataModel;
use scrutiny_collection::static_packages::StaticPkgsCollection;
use scrutiny_collection::zbi::Zbi;
use scrutiny_utils::artifact::{ArtifactReader, FileArtifactReader};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::str;
use std::sync::Arc;
use url::Url;

// The root v2 component manifest.
pub const ROOT_RESOURCE: &str = "meta/root.cm";

struct StaticPackageDescription<'a> {
    pub name: &'a PackageName,
    pub variant: Option<&'a PackageVariant>,
    pub merkle: &'a Hash,
}

impl<'a> StaticPackageDescription<'a> {
    fn new(name: &'a PackageName, variant: Option<&'a PackageVariant>, merkle: &'a Hash) -> Self {
        Self { name, variant, merkle }
    }

    /// Definitions match when:
    /// 1. Package definition has no hash (warning emitted above), or hashes match;
    /// 2. Names match;
    /// 3. A variant is missing (warning emitted above), or variants match.
    fn matches(&self, pkg: &PackageDefinition) -> bool {
        let url = AbsolutePackageUrl::new(
            pkg.url.repository().clone(),
            self.name.clone(),
            self.variant.map(PackageVariant::clone),
            Some(self.merkle.clone()),
        );
        let url_match = match_absolute_pkg_urls(&url, &pkg.url);
        if url_match == PkgUrlMatch::WeakMatch {
            warn!(
                StaticPackageDescription_url:% = url,
                PackageDefinition_url:% = pkg.url;
                "Lossy match of absolute package URLs",
            );
        }
        url_match != PkgUrlMatch::NoMatch
    }
}

/// The PackageDataResponse contains all of the core model information extracted
/// from the Fuchsia Archive (.far) packages from the current build.
pub struct PackageDataResponse {
    pub components: HashMap<Url, Component>,
    pub packages: Vec<Package>,
    pub manifests: Vec<Manifest>,
}

impl PackageDataResponse {
    pub fn new(
        components: HashMap<Url, Component>,
        packages: Vec<Package>,
        manifests: Vec<Manifest>,
    ) -> Self {
        Self { components, packages, manifests }
    }
}

/// The PackageDataCollector is a core collector in Scrutiny that is
/// responsible for extracting data from Fuchsia Archives (.far). This collector
/// scans every single package and extracts all of the manifests and files.
/// Using this raw data it constructs all the routes and components in the
/// model.
#[derive(Default)]
pub struct PackageDataCollector {}

impl PackageDataCollector {
    /// Retrieves the set of packages from the current target build returning
    /// them sorted by package url.
    fn get_packages(package_reader: &mut Box<dyn PackageReader>) -> Result<Vec<PackageDefinition>> {
        let package_urls =
            package_reader.read_package_urls().context("Failed to read listing of package URLs")?;
        let mut pkgs = package_urls
            .into_iter()
            .map(|pkg_url| package_reader.read_package_definition(&pkg_url))
            .collect::<Result<Vec<PackageDefinition>>>()
            .context("Failed to read package definition")?;
        pkgs.sort_by(|lhs, rhs| lhs.url.name().cmp(&rhs.url.name()));
        Ok(pkgs)
    }

    fn get_non_bootfs_pkg_source<'a>(
        pkg: &'a PackageDefinition,
        static_pkgs: &'a Option<Vec<StaticPackageDescription<'a>>>,
    ) -> Result<ComponentSource> {
        let pkg_merkle = pkg.url.hash().ok_or_else(||
            anyhow!("Unable to report precise component source from package URL that is missing package hash: {}", pkg.url)
        )?;

        if static_pkgs.is_none() {
            return Ok(ComponentSource::Package(pkg_merkle.clone()));
        }

        for static_pkg in static_pkgs.as_ref().unwrap().iter() {
            if static_pkg.matches(pkg) {
                return Ok(ComponentSource::StaticPackage(static_pkg.merkle.clone()));
            }
        }

        Ok(ComponentSource::Package(pkg_merkle.clone()))
    }

    fn get_static_pkgs<'a>(
        static_pkgs_result: &'a Result<Arc<StaticPkgsCollection>>,
    ) -> Option<Vec<StaticPackageDescription<'a>>> {
        static_pkgs_result
            .as_ref()
            .ok()
            .map(|result| {
                // Collection is only meaningful if there are static packages and no errors.
                if result.static_pkgs.is_some() && result.errors.len() == 0 {
                    Some(
                        result
                            .static_pkgs
                            .as_ref()
                            .unwrap()
                            .iter()
                            .map(|((name, variant), merkle)| {
                                StaticPackageDescription::new(name, variant.as_ref(), merkle)
                            })
                            .collect(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or(None)
    }

    fn get_static_pkg_deps(
        static_pkgs_result: &Result<Arc<StaticPkgsCollection>>,
    ) -> HashSet<PathBuf> {
        static_pkgs_result.as_ref().ok().map(|result| result.deps.clone()).unwrap_or(HashSet::new())
    }

    /// Extracts all of the components and manifests from a package.
    fn extract_package_data<'a>(
        component_id: &mut i32,
        components: &mut HashMap<Url, Component>,
        manifests: &mut Vec<Manifest>,
        pkg: &PackageDefinition,
        source: &ComponentSource,
    ) -> Result<()> {
        // Extract components from the packages.
        for (path, cm) in &pkg.cms {
            if !is_cf_v2_manifest(path) {
                continue;
            }

            let path_str = path.to_str().ok_or_else(|| {
                anyhow!("Cannot format component manifest path as string: {:?}", path)
            })?;
            *component_id += 1;
            let url = AbsoluteComponentUrl::from_package_url_and_resource(
                pkg.url.clone(),
                path_str.to_string(),
            )
            .with_context(|| {
                format!(
                    r#"Failed to apply resource path "{}" to package URL "{}""#,
                    path_str, pkg.url
                )
            })?;
            let url = Url::parse(&url.to_string()).with_context(|| {
                format!("Failed to convert package URL to standard URL: {}", url)
            })?;
            let component_url = cm_types::Url::new(&url.to_string()).with_context(|| {
                format!("Failed to convert package URL to component URL: {}", url)
            })?;

            components.insert(
                url.clone(),
                Component { id: *component_id, url: component_url, source: source.clone() },
            );

            let cf_manifest = {
                if let ComponentManifest::Version2(decl_bytes) = &cm {
                    let cm_base64 = BASE64_STANDARD.encode(&decl_bytes);
                    let mut cvf_bytes = None;

                    if let Ok(cm_decl) = unpersist::<fdecl::Component>(&decl_bytes) {
                        if let Err(err) =
                            cm_fidl_validator::validate(&cm_decl, &mut DirectedGraph::new())
                        {
                            warn!(err:%, url:%; "Invalid cm");
                        } else {
                            if let Some(schema) = cm_decl.config {
                                match schema
                                    .value_source
                                    .as_ref()
                                    .context("getting value source from config schema")?
                                {
                                    fdecl::ConfigValueSource::PackagePath(pkg_path) => {
                                        cvf_bytes = Some(
                                            pkg.cvfs
                                                .get(pkg_path)
                                                .context("getting config values from package")?
                                                .clone(),
                                        );
                                    }
                                    other => {
                                        warn!("unsupported config value source {:?}", other)
                                    }
                                }
                            }
                        }
                    } else {
                        warn!(url:%; "cm failed to be decoded");
                    }
                    Manifest {
                        component_id: *component_id,
                        manifest: ManifestData { cm_base64, cvf_bytes },
                    }
                } else {
                    Manifest {
                        component_id: *component_id,
                        manifest: ManifestData { cm_base64: String::from(""), cvf_bytes: None },
                    }
                }
            };
            manifests.push(cf_manifest);
        }
        Ok(())
    }

    /// Extracts all the components and manifests from the ZBI.
    fn extract_zbi_data(
        component_id: &mut i32,
        components: &mut HashMap<Url, Component>,
        manifests: &mut Vec<Manifest>,
        zbi: &Zbi,
    ) -> Result<()> {
        Self::extract_bootfs_packaged_data(component_id, components, manifests, zbi)?;

        for (file_name, file_data) in &zbi.bootfs_files.bootfs_files {
            if file_name.ends_with(".cm") {
                Self::extract_bootfs_unpackaged_data(
                    component_id,
                    components,
                    manifests,
                    zbi,
                    file_name,
                    file_data,
                )?;
            }
        }

        Ok(())
    }

    fn extract_bootfs_unpackaged_data(
        component_id: &mut i32,
        components: &mut HashMap<Url, Component>,
        manifests: &mut Vec<Manifest>,
        zbi: &Zbi,
        file_name: &str,
        file_data: &Vec<u8>,
    ) -> Result<()> {
        info!(file_name:%; "Extracting bootfs manifest");
        let url = fuchsia_url::boot::AbsoluteComponentUrl::new(
            None,
            file_name.parse().with_context(|| {
                format!("file_name is an invalid resource path {:?}", file_name)
            })?,
        );
        let url = Url::parse(&url.to_string())
            .with_context(|| format!("Failed to convert boot URL to standard URL: {}", url))?;
        let component_url = cm_types::Url::new(&url.to_string())
            .with_context(|| format!("Failed to convert boot URL to component URL: {}", url))?;

        Self::extract_bootfs_data(
            component_id,
            components,
            manifests,
            &zbi.bootfs_files.bootfs_files,
            &url,
            component_url,
            file_name,
            file_data,
        )
    }

    fn extract_bootfs_packaged_data(
        component_id: &mut i32,
        components: &mut HashMap<Url, Component>,
        manifests: &mut Vec<Manifest>,
        zbi: &Zbi,
    ) -> Result<()> {
        if let Some(package_index) = &zbi.bootfs_packages.bootfs_pkgs {
            // TODO(https://fxbug.dev/326308365): Delete package variant concept.
            // We leave the `variant` out of the component URL that is used in the manifest because
            // SWD is in the long, slow process of deleting the variant concept, and as part of that
            // process the variant is always zero and component and package resolvers will insert
            // a variant of zero if given a URL without a variant.
            for ((name, _variant), merkle) in package_index {
                let package_path = format!("blob/{}", merkle);
                let far_cursor =
                    Cursor::new(zbi.bootfs_files.bootfs_files.get(&package_path).ok_or_else(
                        || anyhow!("Zbi does not contain meta.far at path: {:?}", package_path),
                    )?);
                let partial_package_def = read_partial_package_definition(far_cursor)?;

                for (path, cm) in &partial_package_def.cms {
                    let path_str = path.to_str().ok_or_else(|| {
                        anyhow!("Cannot format component manifest path as string: {:?}", path)
                    })?;
                    match cm {
                        ComponentManifest::Version2(bytes) => {
                            let url = fuchsia_url::boot::AbsoluteComponentUrl::new(
                                Some(name.clone().into()),
                                path_str.parse().with_context(|| {
                                    format!(
                                        "A component manifest path, '{:?}', in the {} {} package \
                                         listed in the ZBI's bootfs package index is invalid",
                                        path_str, name, merkle
                                    )
                                })?,
                            );
                            let url = Url::parse(&url.to_string()).with_context(|| {
                                format!("Failed to convert boot URL to standard URL: {}", url)
                            })?;
                            let component_url =
                                cm_types::Url::new(&url.to_string()).with_context(|| {
                                    format!("Failed to convert boot URL to component URL: {}", url)
                                })?;
                            Self::extract_bootfs_data(
                                component_id,
                                components,
                                manifests,
                                &partial_package_def.cvfs,
                                &url,
                                component_url,
                                path_str,
                                bytes,
                            )?;
                        }
                        _ => anyhow::bail!("Bootfs only supports V2 components."),
                    }
                }
            }
        }

        Ok(())
    }

    fn extract_bootfs_data(
        component_id: &mut i32,
        components: &mut HashMap<Url, Component>,
        manifests: &mut Vec<Manifest>,
        cvf_source: &HashMap<String, Vec<u8>>,
        url: &Url,
        component_url: cm_types::Url,
        file_name: &str,
        file_data: &Vec<u8>,
    ) -> Result<()> {
        info!(file_name:%; "Extracting bootfs manifest");
        let cm_base64 = BASE64_STANDARD.encode(&file_data);
        if let Ok(cm_decl) = unpersist::<fdecl::Component>(&file_data) {
            if let Err(err) = cm_fidl_validator::validate(&cm_decl, &mut DirectedGraph::new()) {
                warn!(file_name:%, err:%; "Invalid cm");
            } else {
                // Retrieve this component's config values, if any.
                let cvf_bytes = if let Some(schema) = cm_decl.config {
                    match schema
                        .value_source
                        .as_ref()
                        .context("getting value source from config schema")?
                    {
                        fdecl::ConfigValueSource::PackagePath(pkg_path) => {
                            cvf_source.get(pkg_path).cloned()
                        }
                        fdecl::ConfigValueSource::Capabilities(_) => None,
                        other => {
                            anyhow::bail!("Unsupported config value source {:?}.", other);
                        }
                    }
                } else {
                    None
                };

                // Add the components directly from the ZBI.
                *component_id += 1;
                components.insert(
                    url.clone(),
                    Component {
                        id: *component_id,
                        url: component_url,
                        source: ComponentSource::ZbiBootfs,
                    },
                );
                manifests.push(Manifest {
                    component_id: *component_id,
                    manifest: ManifestData { cm_base64, cvf_bytes },
                });
            }
        }

        Ok(())
    }

    /// Function to build the component graph model out of the packages and services retrieved
    /// by this collector.
    fn extract<'a>(
        fuchsia_packages: Vec<PackageDefinition>,
        static_pkgs: &'a Option<Vec<StaticPackageDescription<'a>>>,
        model: Arc<DataModel>,
    ) -> Result<PackageDataResponse> {
        let mut components: HashMap<Url, Component> = HashMap::new();
        let mut packages: Vec<Package> = Vec::new();
        let mut manifests: Vec<Manifest> = Vec::new();

        // Iterate through all served packages, for each manifest they define, create a node.
        let mut component_id = 0;
        info!(total = fuchsia_packages.len(); "Found package");
        for pkg in fuchsia_packages.iter() {
            info!(url:% = pkg.url; "Extracting package");
            let merkle = pkg.url.hash().ok_or_else(||
                anyhow!("Unable to extract precise package information from URL without package hash: {}", pkg.url)
            )?.clone();
            let package = Package {
                name: pkg.url.name().clone(),
                variant: pkg.url.variant().map(|variant| variant.clone()),
                merkle,
                contents: pkg.contents.clone(),
                meta: pkg.meta.clone(),
            };
            packages.push(package);

            let source = Self::get_non_bootfs_pkg_source(pkg, static_pkgs)?;
            Self::extract_package_data(
                &mut component_id,
                &mut components,
                &mut manifests,
                &pkg,
                &source,
            )?;
        }

        match model.get() {
            Ok(zbi) => {
                Self::extract_zbi_data(&mut component_id, &mut components, &mut manifests, &zbi)?;
            }
            Err(err) => {
                warn!("{err}");
            }
        }

        info!(components = components.len(), manifests = manifests.len(); "");

        Ok(PackageDataResponse::new(components, packages, manifests))
    }

    pub fn collect_with_reader(
        mut package_reader: Box<dyn PackageReader>,
        artifact_reader: Box<dyn ArtifactReader>,
        model: Arc<DataModel>,
    ) -> Result<()> {
        let served_packages =
            Self::get_packages(&mut package_reader).context("Failed to read packages listing")?;
        info!(packages = served_packages.len(); "Done collecting. Found listed in the sys realm");

        let static_pkgs_result = model.get();
        let static_pkgs = Self::get_static_pkgs(&static_pkgs_result);
        let response = PackageDataCollector::extract(served_packages, &static_pkgs, model.clone())?;

        let mut model_comps = vec![];
        for (_, val) in response.components.into_iter() {
            model_comps.push(val);
        }

        // TODO: Collections need to forward deps to controllers.
        model.set(Components::new(model_comps)).context("Failed to store components in model")?;
        model.set(Packages::new(response.packages)).context("Failed to store packages in model")?;
        model
            .set(Manifests::new(response.manifests))
            .context("Failed to store manifests in model")?;

        let mut deps = Self::get_static_pkg_deps(&static_pkgs_result);
        for dep in package_reader.get_deps().into_iter() {
            deps.insert(dep);
        }
        for dep in artifact_reader.get_deps().into_iter() {
            deps.insert(dep);
        }
        model.set(CoreDataDeps::new(deps)).context("Failed to store core data deps")?;

        Ok(())
    }

    /// Collects and builds a DAG of component nodes (with manifests) and routes that
    /// connect the nodes.
    pub fn collect(&self, model: Arc<DataModel>) -> Result<()> {
        let model_config = model.config();
        let blobs_directory = &model_config.blobs_directory();
        let artifact_reader_for_artifact_reader = FileArtifactReader::new(
            &PathBuf::new(),
            blobs_directory,
            model_config.delivery_blob_type,
        );
        let artifact_reader_for_package_reader = artifact_reader_for_artifact_reader.clone();

        let package_reader: Box<dyn PackageReader> = Box::new(PackagesFromUpdateReader::new(
            &model_config.update_package_path(),
            Box::new(artifact_reader_for_package_reader),
        ));

        Self::collect_with_reader(
            package_reader,
            Box::new(artifact_reader_for_artifact_reader),
            model,
        )?;

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use fidl::persist;

    use super::{PackageDataCollector, StaticPackageDescription};
    use crate::package::{Component, ComponentSource};
    use crate::package_reader::PackageReader;
    use crate::package_test_utils::{
        MockPackageReader, create_model, create_test_cm_map, create_test_package_with_cms,
        create_test_package_with_contents,
    };
    use crate::package_types::{PackageDefinition, PartialPackageDefinition};
    use base64::engine::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use cm_rust::{ComponentDecl, NativeIntoFidl};
    use fidl_fuchsia_component_decl as fdecl;
    use fuchsia_hash::{HASH_SIZE, Hash};
    use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
    use fuchsia_url::{PackageName, PackageVariant};
    use maplit::{hashmap, hashset};
    use scrutiny_collection::core::{Components, CoreDataDeps, ManifestData, Manifests, Packages};
    use scrutiny_collection::zbi::Zbi;
    use scrutiny_testing::artifact::MockArtifactReader;
    use scrutiny_testing::fake::fake_component_src_pkg;
    use scrutiny_utils::artifact::ArtifactReader;
    use scrutiny_utils::bootfs::{BootfsFileIndex, BootfsPackageIndex};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::str::FromStr;
    use std::sync::Arc;
    use url::Url;

    fn make_v2_manifest_data(decl: ComponentDecl) -> ManifestData {
        let decl_fidl: fdecl::Component = decl.native_into_fidl();
        let cm_base64 = BASE64_STANDARD.encode(&persist(&decl_fidl).unwrap());
        ManifestData { cm_base64, cvf_bytes: None }
    }

    fn default_pkg() -> PackageDefinition {
        PackageDefinition {
            url: "fuchsia-pkg://fuchsia.com/default/0?hash=0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
            meta: HashMap::new(),
            contents: HashMap::new(),
            cms: HashMap::new(),
            cvfs: HashMap::new(),
        }
    }

    fn empty_update_pkg() -> PartialPackageDefinition {
        PartialPackageDefinition {
            meta: HashMap::new(),
            contents: HashMap::new(),
            cms: HashMap::new(),
            cvfs: HashMap::new(),
        }
    }

    #[fuchsia::test]
    fn test_static_pkgs_matches() {
        let pkg_def_with_variant = PackageDefinition {
            url: "fuchsia-pkg://fuchsia.com/alpha-beta_gamma9/0?hash=0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
            ..default_pkg()
        };
        let pkg_def_without_variant = PackageDefinition {
            url: "fuchsia-pkg://fuchsia.com/alpha-beta_gamma9?hash=0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
            ..default_pkg()
        };
        // Match.
        assert!(
            StaticPackageDescription::new(
                &PackageName::from_str("alpha-beta_gamma9").unwrap(),
                Some(&PackageVariant::zero()),
                &Hash::from([0u8; HASH_SIZE])
            )
            .matches(&pkg_def_with_variant)
        );
        // Match with self variant None, input variant Some.
        assert!(
            StaticPackageDescription::new(
                &PackageName::from_str("alpha-beta_gamma9").unwrap(),
                None,
                &Hash::from([0u8; HASH_SIZE])
            )
            .matches(&pkg_def_with_variant)
        );
        // Match with self variant Some, input variant None.
        assert!(
            StaticPackageDescription::new(
                &PackageName::from_str("alpha-beta_gamma9").unwrap(),
                Some(&PackageVariant::zero()),
                &Hash::from([0u8; HASH_SIZE])
            )
            .matches(&pkg_def_without_variant)
        );
        // Variant mismatch.
        assert!(
            StaticPackageDescription::new(
                &PackageName::from_str("alpha-beta_gamma9").unwrap(),
                Some(&PackageVariant::from_str("1").unwrap()),
                &Hash::from([0u8; HASH_SIZE])
            )
            .matches(&pkg_def_with_variant)
                == false
        );
        // Merkle mismatch.
        assert!(
            StaticPackageDescription::new(
                &PackageName::from_str("alpha").unwrap(),
                Some(&PackageVariant::zero()),
                &Hash::from([1u8; HASH_SIZE])
            )
            .matches(&pkg_def_with_variant)
                == false
        );
    }

    fn count_sources(components: HashMap<Url, Component>) -> (usize, usize, usize) {
        let mut zbi_bootfs_count = 0;
        let mut package_count = 0;
        let mut static_package_count = 0;
        for (_, comp) in components {
            match comp.source {
                ComponentSource::ZbiBootfs => {
                    zbi_bootfs_count += 1;
                }
                ComponentSource::Package(_) => {
                    package_count += 1;
                }
                ComponentSource::StaticPackage(_) => {
                    static_package_count += 1;
                }
            }
        }
        (zbi_bootfs_count, package_count, static_package_count)
    }

    #[fuchsia::test]
    fn test_extract_with_invalid_cm_paths_creates_empty_graph() {
        // Create a single test package with invalid cm paths
        let cms = create_test_cm_map(vec![
            (PathBuf::from("foo/bar.cm"), vec![0, 1]),
            (PathBuf::from("meta/baz"), vec![1, 0]),
        ]);
        let pkg = create_test_package_with_cms(PackageName::from_str("foo").unwrap(), None, cms);
        let served = vec![pkg];
        let model = create_model();
        model
            .set(Zbi {
                deps: HashSet::default(),
                sections: vec![],
                bootfs_files: BootfsFileIndex::default(),
                bootfs_packages: BootfsPackageIndex::default(),
                cmdline: vec![],
            })
            .unwrap();

        let response = PackageDataCollector::extract(served, &None, model).unwrap();

        assert_eq!(0, response.components.len());
        assert_eq!(0, response.manifests.len());
        assert_eq!(1, response.packages.len());
        // 0 zbi/bootfs, 0 (non-static) package, 0 static packages.
        assert_eq!((0, 0, 0), count_sources(response.components));
    }

    #[fuchsia::test]
    fn test_extract_with_cm() {
        let cms = create_test_cm_map(vec![(PathBuf::from("meta/foo.cm"), vec![])]);
        let pkg = create_test_package_with_cms(PackageName::from_str("foo").unwrap(), None, cms);
        let served = vec![pkg];
        let model = create_model();
        model
            .set(Zbi {
                deps: HashSet::default(),
                sections: vec![],
                bootfs_files: BootfsFileIndex::default(),
                bootfs_packages: BootfsPackageIndex::default(),
                cmdline: vec![],
            })
            .unwrap();

        let response = PackageDataCollector::extract(served, &None, model).unwrap();

        assert_eq!(1, response.components.len());
        assert_eq!(1, response.manifests.len());
        assert_eq!(1, response.packages.len());
    }

    #[fuchsia::test]
    fn test_collect_clears_data_model_before_adding_new() {
        let mut mock_pkg_reader = Box::new(MockPackageReader::new());
        let model = create_model();
        // Put some "previous" content into the model.
        {
            let mut comps = vec![];
            comps.push(Component {
                id: 1,
                url: cm_types::Url::new("fuchsia-pkg://test.fuchsia.com/test?hash=0000000000000000000000000000000000000000000000000000000000000000#meta/test.component.cm").unwrap(),
                source: ComponentSource::ZbiBootfs,
            });
            comps.push(Component {
                id: 1,
                url: cm_types::Url::new("fuchsia-pkg://test.fuchsia.com/test?hash=0000000000000000000000000000000000000000000000000000000000000000#meta/foo.bar.cm").unwrap(),
                source: fake_component_src_pkg(),
            });
            model.set(Components { entries: comps }).unwrap();

            let mut manis = vec![];
            manis.push(scrutiny_collection::core::Manifest {
                component_id: 1,
                manifest: make_v2_manifest_data(ComponentDecl { ..ComponentDecl::default() }),
            });
            manis.push(scrutiny_collection::core::Manifest {
                component_id: 2,
                manifest: make_v2_manifest_data(ComponentDecl { ..ComponentDecl::default() }),
            });
            model.set(Manifests { entries: manis }).unwrap();
        }

        let cms = create_test_cm_map(vec![(PathBuf::from("meta/bar.cm"), vec![])]);
        let pkg = create_test_package_with_cms(PackageName::from_str("foo").unwrap(), None, cms);
        let pkg_urls = vec![pkg.url.clone().pinned().unwrap()];
        mock_pkg_reader.append_update_package(pkg_urls, empty_update_pkg());
        mock_pkg_reader.append_pkg_def(pkg);

        let mock_artifact_reader = Box::new(MockArtifactReader::new());
        PackageDataCollector::collect_with_reader(
            mock_pkg_reader,
            mock_artifact_reader,
            Arc::clone(&model),
        )
        .unwrap();

        // Ensure the model reflects only the latest collection.
        let comps = &model.get::<Components>().unwrap().entries;
        let manis = &model.get::<Manifests>().unwrap().entries;
        // There is 1 component, 1 manifest (for the defined package), and 0 routes
        assert_eq!(comps.len(), 1);
        assert_eq!(manis.len(), 1);
    }

    #[fuchsia::test]
    fn test_malformed_zbi() {
        let mut contents = HashMap::new();
        contents.insert(PathBuf::from("zbi"), Hash::from([0u8; HASH_SIZE]));
        let pkg = create_test_package_with_contents(
            PackageName::from_str("update").unwrap(),
            Some(PackageVariant::zero()),
            contents,
        );
        let served = vec![pkg];
        let model = create_model();
        model
            .set(Zbi {
                deps: HashSet::default(),
                sections: vec![],
                bootfs_files: BootfsFileIndex::default(),
                bootfs_packages: BootfsPackageIndex::default(),
                cmdline: vec![],
            })
            .unwrap();

        PackageDataCollector::extract(served, &None, model).unwrap();
    }

    #[fuchsia::test]
    fn test_packages_sorted() {
        let mut mock_pkg_reader = Box::new(MockPackageReader::new());
        let model = create_model();

        let cms_0 = create_test_cm_map(vec![(PathBuf::from("meta/foo.cm"), vec![])]);
        let pkg_0 =
            create_test_package_with_cms(PackageName::from_str("foo").unwrap(), None, cms_0);
        let pkg_0_url = pkg_0.url.clone();
        mock_pkg_reader.append_pkg_def(pkg_0);

        let cms_1 = create_test_cm_map(vec![(PathBuf::from("meta/bar.cm"), vec![])]);
        let pkg_1 =
            create_test_package_with_cms(PackageName::from_str("bar").unwrap(), None, cms_1);
        let pkg_1_url = pkg_1.url.clone();
        mock_pkg_reader.append_pkg_def(pkg_1);

        let pkg_urls = vec![pkg_0_url.pinned().unwrap(), pkg_1_url.pinned().unwrap()];
        mock_pkg_reader.append_update_package(pkg_urls, empty_update_pkg());

        let mock_artifact_reader = MockArtifactReader::new();
        let package_reader: Box<dyn PackageReader> = mock_pkg_reader;
        let artifact_reader: Box<dyn ArtifactReader> = Box::new(mock_artifact_reader);

        PackageDataCollector::collect_with_reader(
            package_reader,
            artifact_reader,
            Arc::clone(&model),
        )
        .unwrap();

        // Test that the packages are in sorted order.
        let packages = &model.get::<Packages>().unwrap().entries;
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name.to_string(), "bar");
        assert_eq!(packages[1].name.to_string(), "foo");
    }

    #[fuchsia::test]
    fn test_deps() {
        let mut mock_pkg_reader = Box::new(MockPackageReader::new());
        let model = create_model();

        let merkle_one = Hash::from([1u8; HASH_SIZE]);
        let merkle_two = Hash::from([2u8; HASH_SIZE]);
        let pkg_url_one = AbsolutePackageUrl::parse(&format!(
            "fuchsia-pkg://test.fuchsia.com/one-two-three?hash={}",
            merkle_one.to_string()
        ))
        .unwrap();
        let pkg_url_two = AbsolutePackageUrl::parse(&format!(
            "fuchsia-pkg://test.fuchsia.com/four-five-six?hash={}",
            merkle_two.to_string()
        ))
        .unwrap();

        let mut mock_artifact_reader = Box::new(MockArtifactReader::new());
        mock_pkg_reader.append_pkg_def(PackageDefinition {
            url: pkg_url_one.clone(),
            meta: hashmap! {},
            contents: hashmap! {},
            cms: hashmap! {},
            cvfs: hashmap! {},
        });
        mock_pkg_reader.append_pkg_def(PackageDefinition {
            url: pkg_url_two.clone(),
            meta: hashmap! {},
            contents: hashmap! {},
            cms: hashmap! {},
            cvfs: hashmap! {},
        });

        let pkg_urls = vec![pkg_url_one.pinned().unwrap(), pkg_url_two.pinned().unwrap()];
        mock_pkg_reader.append_update_package(pkg_urls, empty_update_pkg());

        // Append a dep to both package and artifact readers.
        let pkg_path: PathBuf = "pkg.far".to_string().into();
        let artifact_path: PathBuf = "artifact".to_string().into();
        mock_pkg_reader.append_dep(pkg_path.clone());
        mock_artifact_reader.append_dep(artifact_path.clone());

        PackageDataCollector::collect_with_reader(
            mock_pkg_reader,
            mock_artifact_reader,
            Arc::clone(&model),
        )
        .unwrap();
        let deps: Arc<CoreDataDeps> = model.get().unwrap();
        assert_eq!(
            deps,
            Arc::new(CoreDataDeps {
                deps: hashset! {
                    pkg_path,
                    artifact_path,
                },
            })
        );
    }
}
