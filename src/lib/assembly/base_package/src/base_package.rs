// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use assembly_constants::PackageDestination;
use assembly_package_list::{PackageList, PackageSetMap, PackageUrlList, WritablePackageList};
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_pkg::package_sets::{AnchoredPackageSetType, PackageSetType};
use fuchsia_pkg::{PackageBuilder, PackageManifest, RelativeTo};
use std::collections::BTreeMap;

/// The path to the static package index file in the `base` package.
const STATIC_PACKAGE_INDEX: &str = "data/static_packages";

/// The path to the cache package index file in the `base` package.
const CACHE_PACKAGE_INDEX: &str = "data/cache_packages.json";

/// The path to the cache package index file in the `base` package.
const ANCHORED_PACKAGE_INDEX: &str = "data/anchored_packages.json";

/// A builder that constructs base packages.
#[derive(Default)]
pub struct BasePackageBuilder {
    // Maps the blob destination -> source.
    contents: BTreeMap<String, String>,
    base_packages: PackageList,
    cache_packages: PackageUrlList,
    anchored_packages: PackageSetMap,
}

impl BasePackageBuilder {
    /// Add all the blobs from `package` into the base package being built.
    pub fn add_files_from_package(&mut self, package: PackageManifest) {
        package.into_blobs().into_iter().filter(|b| b.path != "meta/").for_each(|b| {
            self.contents.insert(b.path, b.source_path);
        });
    }

    /// Add the `package` to the list of base packages, which is then added to
    /// base package as file `data/static_packages`.
    pub fn add_base_package(&mut self, package: PackageManifest) -> Result<()> {
        self.base_packages.add_package(package)
    }

    /// Add the `package` to the list of cache packages, which is then added to
    /// base package as file `data/cache_packages`.
    pub fn add_cache_package(&mut self, package: PackageManifest) -> Result<()> {
        self.cache_packages.add_package(package)
    }

    /// Add the `package` to the list of anchored packages, which is then added to
    /// base package as file `data/anchored_packages.json`.
    pub fn add_anchored_package(
        &mut self,
        package: PackageManifest,
        package_set_type: AnchoredPackageSetType,
    ) -> Result<()> {
        self.anchored_packages
            .add_package_with_package_set(package, Some(PackageSetType::Anchored(package_set_type)))
    }

    /// Build the base package and write the bytes to `out`.
    ///
    /// Intermediate files will be written to the directory specified by
    /// `gendir`.
    pub fn build(
        self,
        gendir: impl AsRef<Utf8Path>,
        name: impl AsRef<str>,
        out: impl AsRef<Utf8Path>,
    ) -> Result<BasePackageBuildResults> {
        let gendir = gendir.as_ref();
        let out = out.as_ref();

        // Write all generated files in a subdir with the name of the package.
        let gendir = gendir.join(name.as_ref());

        let Self { contents, base_packages, cache_packages, anchored_packages } = self;

        // Capture the generated files
        let mut generated_files = BTreeMap::new();

        // Generate the base and cache package lists.
        let (dest, path) = base_packages.write_index_file(&gendir, "base", STATIC_PACKAGE_INDEX)?;
        generated_files.insert(dest, path);

        let (dest, path) =
            cache_packages.write_index_file(&gendir, "cache", CACHE_PACKAGE_INDEX)?;
        generated_files.insert(dest, path);

        let (dest, path) =
            anchored_packages.write_index_file(&gendir, "anchored", ANCHORED_PACKAGE_INDEX)?;
        generated_files.insert(dest, path);

        // Construct the list of blobs in the base package that lives outside of the meta.far.
        let mut external_contents = contents;
        for (destination, source) in &generated_files {
            external_contents.insert(destination.clone(), source.clone());
        }

        // It's not totally clear what the ABI revision means for the
        // system-image package. It isn't checked anywhere. Regardless, it's
        // never produced by assembly tools from one Fuchsia release and then
        // read by binaries from another Fuchsia release, so the ABI revision
        // for platform components seems appropriate.
        //
        // TODO(https://fxbug.dev/329125882): Clarify what this means.
        //
        // Also: all base packages are named 'system-image' internally, for
        // consistency on the platform.
        let mut builder =
            PackageBuilder::new_platform_internal_package(PackageDestination::Base.to_string());
        // However, they can have different published names.  And the name here
        // is the name to publish it under (and to include in the generated
        // package manifest).
        builder.published_name(name);

        for (destination, source) in &external_contents {
            builder.add_file_as_blob(destination, source)?;
        }
        let manifest_path = gendir.join("package_manifest.json");
        builder.manifest_path(manifest_path.clone());
        builder.manifest_blobs_relative_to(RelativeTo::File);
        builder.build(gendir.as_std_path(), out.as_std_path())?;

        Ok(BasePackageBuildResults {
            contents: external_contents,
            base_packages,
            cache_packages,
            anchored_packages,
            generated_files,
            manifest_path,
        })
    }
}

/// The results of building the `base` package.
///
/// These are based on the information that the builder is configured with, and
/// then augmented with the operations that the `BasePackageBuilder::build()`
/// fn performs, including an extra additions or removals.
///
/// This provides an audit trail of "what was created".
pub struct BasePackageBuildResults {
    // Maps the blob destination -> source.
    pub contents: BTreeMap<String, String>,
    pub base_packages: PackageList,
    pub cache_packages: PackageUrlList,
    pub anchored_packages: PackageSetMap,

    /// The paths to the files generated by the builder.
    pub generated_files: BTreeMap<String, String>,

    pub manifest_path: Utf8PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_test_util::generate_test_manifest;
    use fuchsia_archive::Utf8Reader;
    use fuchsia_hash::Hash;
    use fuchsia_pkg::package_sets::AnchoredPackageSetType;
    use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
    use std::fs::File;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn build_with_unsupported_packages() {
        let mut builder = BasePackageBuilder::default();
        assert!(builder.add_base_package(generate_test_manifest("system_image", None)).is_err());
        assert!(builder.add_base_package(generate_test_manifest("update", None)).is_err());
    }

    #[test]
    fn build() {
        let gendir_tmp = TempDir::new().unwrap();
        let gendir = Utf8Path::from_path(gendir_tmp.path()).unwrap();
        let far_path = gendir.join("base.far");

        // Build the base package with an extra file, a base package, and a cache package.
        let mut builder = BasePackageBuilder::default();
        let test_file = NamedTempFile::new().unwrap();
        builder.add_files_from_package(generate_test_manifest("package", Some(test_file.path())));
        builder.add_base_package(generate_test_manifest("base_package", None)).unwrap();
        builder.add_cache_package(generate_test_manifest("cache_package", None)).unwrap();
        builder
            .add_anchored_package(
                generate_test_manifest("automatic_anchored_package", None),
                AnchoredPackageSetType::Automatic,
            )
            .unwrap();

        let build_results = builder.build(&gendir, "system_image", &far_path).unwrap();

        // The following asserts lead up to the final one, catching earlier failure points where it
        // can be more obvious as to why the test is failing, as the hashes themselves are opaque.

        // Verify the package list intermediate structures.
        assert_eq!(
            vec![("base_package/0".to_string(), Hash::from([0u8; 32]))],
            build_results.base_packages
        );
        let url: PinnedAbsolutePackageUrl = "fuchsia-pkg://testrepository.com/cache_package/0\
             ?hash=0000000000000000000000000000000000000000000000000000000000000000"
            .parse()
            .unwrap();
        assert_eq!(vec![&url], build_results.cache_packages.get_packages());

        let mut expected_anchored = PackageSetMap::default();
        expected_anchored
            .insert(
                Some("testrepository.com"),
                "automatic_anchored_package/0",
                Hash::from([0u8; 32]),
                Some(PackageSetType::Anchored(AnchoredPackageSetType::Automatic)),
            )
            .unwrap();
        assert_eq!(expected_anchored, build_results.anchored_packages);

        // Inspect the generated files to verify their contents.
        let gen_static_index = build_results.generated_files.get("data/static_packages").unwrap();
        assert_eq!(
            "base_package/0=0000000000000000000000000000000000000000000000000000000000000000\n",
            std::fs::read_to_string(gen_static_index).unwrap()
        );

        let gen_cache_index =
            build_results.generated_files.get("data/cache_packages.json").unwrap();
        let cache_packages_json = r#"{"version":"1","content":["fuchsia-pkg://testrepository.com/cache_package/0?hash=0000000000000000000000000000000000000000000000000000000000000000"]}"#;

        assert_eq!(cache_packages_json, std::fs::read_to_string(gen_cache_index).unwrap());

        // Validate that the generated files are in the contents.
        for (generated_file, _) in &build_results.generated_files {
            assert!(
                build_results.contents.contains_key(generated_file),
                "Unable to find generated file in base package contents: {}",
                generated_file
            );
        }

        // Read the output and ensure it contains the right files (and their hashes)
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(br#"{"name":"system_image","version":"0"}"#, &*package);
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "\
            data/anchored_packages.json=2fdb5ae5a453c197b7b13c1b736116a0841486634a8beaed679809877c15e128\n\
            data/cache_packages.json=f89b7cf2e2b97e09d407036524c62bf67833a4d573110ad39cfe2c8d523c07c0\n\
            data/file.txt=15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b\n\
            data/static_packages=2d86ccb37d003499bdc7bdd933428f4b83d9ed224d1b64ad90dc257d22cff460\n\
        "
        .to_string();
        assert_eq!(expected_contents, contents);
    }
}
