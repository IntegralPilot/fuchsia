// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_util::{NamedMap, fast_copy};
use camino::Utf8PathBuf;
use fuchsia_pkg::{BlobInfo, MetaPackage, PackageManifest, PackageManifestBuilder, SubpackageInfo};
use std::collections::{BTreeMap, BTreeSet};

/// A builder-like struct that copies packages, along with their subpackages,
/// and all blobs into a set of destination directories:
///   1. package (manifest) directory
///   2. subpackage (manifest) directory
///   3. blobstore
///
/// All subpackages and blobs are deduplicated and then copied to their
/// respective directories using their merkle (content identity).
///
/// Packages are copied to the packages directory by the name in their package
/// manifest.
///
pub struct PackageCopier {
    /// This is the directory that all the packages will be written to.
    packages_dir: Utf8PathBuf,

    /// This is the directory that all the sub_packages will be written to:
    subpackages_dir: Utf8PathBuf,

    /// This is where all the blobs will be copied to.
    blobstore: Utf8PathBuf,

    /// These are the package manifests that need to be written out to the
    /// packages dir, keyed by package name.  Duplicate keys with different
    /// values are an error.
    package_manifests: NamedMap<String, PackageManifest>,

    /// These are the subpackage manifests that need to be written out to the
    /// packages dir, keyed by package meta.far merkele.  Duplicates are ignored.
    subpackage_manifests: BTreeMap<fuchsia_merkle::Hash, PackageManifest>,

    /// These are source paths for all the blobs that all need to be copied,
    /// keyed by blob merkle.
    blobs: BTreeMap<fuchsia_merkle::Hash, Utf8PathBuf>,

    /// These are the files read or copied during the copy operation.
    tracked_inputs: BTreeSet<Utf8PathBuf>,
}

impl PackageCopier {
    /// Create a new PackageCopier that will copy packages into the given
    /// directories.
    ///
    /// - `packages_dir` - Where package manifests are written (by name)
    /// - `subpackages_dir` - Where package manifests for all subpackages are
    ///                       written, by the merkle of the subpackage
    /// - `blobstore` - Where all blobs are copied to, by merkle.
    ///
    pub fn new(
        packages_dir: impl Into<Utf8PathBuf>,
        subpackages_dir: impl Into<Utf8PathBuf>,
        blobstore: impl Into<Utf8PathBuf>,
    ) -> Self {
        Self {
            // Constant fields for the life of the struct.
            packages_dir: packages_dir.into(),
            subpackages_dir: subpackages_dir.into(),
            blobstore: blobstore.into(),

            // Internal state that items to copy are accumulated in.
            package_manifests: NamedMap::new("package manifests"),
            subpackage_manifests: Default::default(),
            blobs: Default::default(),
            tracked_inputs: Default::default(),
        }
    }

    /// Add the package at the given manifest path to the set of packages to
    /// copy, and return the new path the package will use.
    pub fn add_package_from_manifest_path<'a>(
        &mut self,
        manifest_path: impl Into<Utf8PathBuf>,
    ) -> Result<Utf8PathBuf> {
        let manifest_path = manifest_path.into();
        let manifest = PackageManifest::try_load_from(&manifest_path)?;
        self.tracked_inputs.insert(manifest_path);

        self.add_package(manifest)
    }

    /// Add the package represented by the given PackageManifest struct to the
    /// set of packages to copy, and return the path that it will be copied to.
    pub fn add_package(&mut self, manifest: PackageManifest) -> Result<Utf8PathBuf> {
        // Add its blobs to the set of blobs to copy.
        self.load_blobs_from(&manifest);
        // Add its subpackages (and their blobs) to the set of subpackages (and
        // blobs) to copy.
        self.load_subpackages_from(&manifest)
            .with_context(|| format!("Loading subpackages from: {}", manifest.name()))?;

        let copied_path = self.packages_dir.join(manifest.name().as_ref());

        // Add the manifest to the set of manifests that will need to be written
        // out with new paths as part of the copy operation.
        self.package_manifests
            .try_insert_unique_ignore_duplicates(manifest.name().to_string(), manifest)?;

        // Return the path the package will be copied to.
        Ok(copied_path)
    }

    /// Given a package manifest, load all of its blobs into the given map.
    fn load_blobs_from<'a>(&mut self, manifest: &'a PackageManifest) {
        for blob in manifest.blobs() {
            self.blobs.entry(blob.merkle).or_insert_with(|| Utf8PathBuf::from(&blob.source_path));
        }
    }

    /// Given a package manifest, load all of its subpackages and blobs (and
    /// recursively, all of the subpackages' subpackages and blobs into the given
    /// maps.
    fn load_subpackages_from<'a>(&mut self, manifest: &'a PackageManifest) -> Result<()> {
        for subpackage in manifest.subpackages() {
            // Multiple packages can subpackage the same package, and this is
            // not an error, but the subpackage is only needed once, so skip the
            // subpackage if has already been seen.
            if !self.subpackage_manifests.contains_key(&subpackage.merkle) {
                self.tracked_inputs.insert(Utf8PathBuf::from(&subpackage.manifest_path));
                let subpackage_manifest = PackageManifest::try_load_from(&subpackage.manifest_path)
                    .with_context(|| {
                        format!("Loading manifest for subpackage: {}", subpackage.name)
                    })?;

                self.load_blobs_from(&subpackage_manifest);
                self.load_subpackages_from(&subpackage_manifest)
                    .with_context(|| format!("Loading subpackage: {}", subpackage.name))?;

                self.subpackage_manifests.insert(subpackage.merkle.clone(), subpackage_manifest);
            }
        }
        Ok(())
    }

    /// Copy all the items that need to be copied.
    ///
    /// Returns the set of files that were read or copied from in the overall
    /// copy operation (including the preparation steps).
    pub fn perform_copy(self) -> Result<BTreeSet<Utf8PathBuf>> {
        let Self {
            package_manifests,
            packages_dir,
            subpackage_manifests,
            subpackages_dir,
            blobs,
            blobstore,
            mut tracked_inputs,
        } = self;

        // Write out each package manifest, altering the subpackage manifest and
        // blob paths to be in their respective directories.
        for (name, manifest) in package_manifests {
            Self::write_relocated_package_manifest(
                name,
                manifest,
                &packages_dir,
                &subpackages_dir,
                &blobstore,
            )
            .context("Copying package manifest")?;
        }

        // Write out each subpackage manifest, altering the subpackage manifest and
        // blob paths to be in their respective directories.
        //
        // Note that the 'name' of the package, in this case, is just the merkle of
        // the meta.far for the package, and the destination is the subpackages_dir
        // and not packages_dir.
        for (merkle, manifest) in subpackage_manifests {
            Self::write_relocated_package_manifest(
                merkle.to_string(),
                manifest,
                &subpackages_dir,
                &subpackages_dir,
                &blobstore,
            )
            .context("Copying package manifest for subpackage")?;
        }

        // Copy all the blobs needed for the above packages and subpackages.
        for (merkle, source_path) in blobs {
            // track the source_path for deps.
            tracked_inputs.insert(source_path.clone());

            // Attempt to hardlink from the blobstore to the source path, falling back
            // to a copy if that fails.
            fast_copy(source_path, blobstore.join(merkle.to_string()))
                .with_context(|| format!("Copying the blob: {}", merkle))?;
        }

        Ok(tracked_inputs)
    }

    /// Write out the package manifest in the given packages dir, using the given
    /// directories for the location of all subpackages and blobs.  Both subpackages
    /// and blobs are identified on the filesystem solely by their merkle.
    ///
    /// NOTE: This does not copy either the subpackages or the blobs themselves.  It
    /// only writes out the package_manifest, using the given locations.
    fn write_relocated_package_manifest(
        name: String,
        manifest: PackageManifest,
        packages_dir: &Utf8PathBuf,
        subpackages_dir: &Utf8PathBuf,
        blobstore: &Utf8PathBuf,
    ) -> Result<()> {
        let internal_name = manifest.name().clone();

        // Pass the package name from the manifest through to the builder, instead of
        // using the published name (the 'name' argument passed to this fn).
        let mut builder = PackageManifestBuilder::new(MetaPackage::from_name_and_variant_zero(
            manifest.name().clone(),
        ));

        // Pass the repository and ABI stamp through if set.
        if let Some(repository) = manifest.repository() {
            builder = builder.repository(repository);
        }
        builder = builder.abi_revision(manifest.abi_revision());

        let (blobs, subpackages) = manifest.into_blobs_and_subpackages();

        // Re-write the blob paths to point at the blobstore
        for blob in blobs {
            let source_path = blobstore.join(blob.merkle.to_string()).to_string();
            builder = builder.add_blob(BlobInfo { source_path, ..blob });
        }

        // Re-write the subpackage manifest paths to point at the subpackage dir
        for subpackage in subpackages {
            let manifest_path = subpackages_dir.join(subpackage.merkle.to_string()).to_string();
            builder = builder.add_subpackage(SubpackageInfo { manifest_path, ..subpackage });
        }

        // And then write out the package to a destination named for the published
        // name of the package.
        let destination_path = packages_dir.join(name);
        builder.build().write_with_relative_paths(&destination_path).with_context(|| {
            format!(
                "Writing relocated package manifest for {} to: {}",
                internal_name, destination_path
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_pkg::PackageBuilder;
    use tempfile::TempDir;

    /// Helper function for creating files that are added to packages as blobs.
    fn make_file(name: &str, package: &str, gendir: &Utf8PathBuf) -> Result<(String, Utf8PathBuf)> {
        let path = format!("{name}.text");
        let source_path = gendir.join(&path);
        let contents = format!("This is {} {}.", package, name);
        std::fs::write(&source_path, &contents)
            .with_context(|| format!("writing file to {}:", &source_path))?;
        Ok((path, source_path))
    }

    /// Helper function for creating packages (with blobs), that can be copied.
    fn make_package(
        name: &str,
        files: &[&str],
        subpackages: &[(&Utf8PathBuf, &PackageManifest)],
        dir: impl Into<Utf8PathBuf>,
    ) -> Result<(Utf8PathBuf, PackageManifest)> {
        let dir: Utf8PathBuf = dir.into();
        let gendir = dir.join(name);
        let metafar_path = gendir.join("meta.far");
        let manifest_path = gendir.join("package_manifest.json");

        std::fs::create_dir_all(&gendir).with_context(|| format!("creating dir: {}", gendir))?;

        let mut builder = PackageBuilder::new_platform_internal_package(name);
        builder.manifest_path(&manifest_path);

        for file in files {
            let (at_path, file) = make_file(file, name, &gendir)
                .with_context(|| format!("making file {} for package {}", file, name))?;
            builder.add_file_as_blob(&at_path, file.to_string())?;
        }
        for (subpackage_manifest_path, subpackage_manifest) in subpackages {
            builder.add_subpackage(
                &subpackage_manifest.name().clone().into(),
                subpackage_manifest.hash(),
                subpackage_manifest_path.into(),
            )?;
        }

        let package_manifest = builder
            .build(&gendir, &metafar_path)
            .with_context(|| format!("building package {}", name))?;

        Ok((manifest_path, package_manifest))
    }

    /// Helper function to easily get the list of files in a directory, for
    /// comparing against the expected ones in a test.
    fn sorted_file_list(dir: &Utf8PathBuf) -> Vec<String> {
        let mut files = std::fs::read_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|entry| {
                format!("{}", entry.unwrap().path().file_name().unwrap().to_string_lossy())
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[test]
    fn test_package_copy() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = Utf8PathBuf::try_from(temp_dir.path().to_owned()).unwrap();

        let src_dir = working_dir.join("src");

        // Create a series of packages
        //
        // package_a/
        //    file_1
        //
        // package_b/
        //    file_1
        //    file_2
        //
        // package_c/
        //    file_1
        //    subpackages: [ package_b ]
        //
        // package_d/
        //    subpackages: [ package_c ]  <- This is to test subpackage recursion
        //

        // This is used to accumulate the list of expected blobs in the blobstore
        // after the copy operation.
        let mut expected_blobs = BTreeSet::new();

        let (package_a_path, package_a) =
            make_package("package_a", &["file_1"], &[], &src_dir).unwrap();

        for blob in package_a.blobs() {
            expected_blobs.insert(blob.merkle);
        }

        let (package_b_path, package_b) =
            make_package("package_b", &["file_1", "file_2"], &[], &src_dir).unwrap();

        for blob in package_b.blobs() {
            expected_blobs.insert(blob.merkle);
        }

        let (package_c_path, package_c) =
            make_package("package_c", &["file_1"], &[(&package_b_path, &package_b)], &src_dir)
                .unwrap();

        for blob in package_c.blobs() {
            expected_blobs.insert(blob.merkle);
        }

        let (package_d_path, package_d) =
            make_package("package_d", &[], &[(&package_c_path, &package_c)], &src_dir).unwrap();

        for blob in package_d.blobs() {
            expected_blobs.insert(blob.merkle);
        }

        let dst_dir = working_dir.join("dst");
        let packages_dir = dst_dir.join("packages");
        let subpackages_dir = dst_dir.join("subpackages");
        let blobstore = dst_dir.join("blobs");

        let mut package_copier = PackageCopier::new(&packages_dir, &subpackages_dir, &blobstore);
        package_copier.add_package_from_manifest_path(&package_a_path).unwrap();
        package_copier.add_package_from_manifest_path(&package_d_path).unwrap();
        let inputs_for_depfile = package_copier.perform_copy().unwrap();

        // Validate that the expected files in are in the destination directories

        assert_eq!(sorted_file_list(&packages_dir), vec!["package_a", "package_d"]);
        let mut expected_subpackages =
            vec![package_b.hash().to_string(), package_c.hash().to_string()];
        expected_subpackages.sort();
        assert_eq!(sorted_file_list(&subpackages_dir), expected_subpackages);
        assert_eq!(
            sorted_file_list(&blobstore),
            expected_blobs.iter().map(ToString::to_string).collect::<Vec<String>>()
        );

        // Spot-check that the package manifest for a copied package points to
        // the correct new paths for subpackages and blobs.
        let copied_package_d = PackageManifest::try_load_from(
            working_dir.join(working_dir.join("dst/packages/package_d")),
        )
        .unwrap();

        // Validate that the package manifest contains the right name and merkle
        assert_eq!(copied_package_d.name().to_string(), "package_d");
        assert_eq!(copied_package_d.hash(), package_d.hash());

        // Validate that the path to the subpackage manifest is the copied
        // manifest, in the dst directory.
        assert_eq!(
            copied_package_d.subpackages().get(0).unwrap().manifest_path,
            working_dir.join(format!("dst/subpackages/{}", package_c.hash()))
        );

        assert_eq!(
            copied_package_d.blobs().get(0).unwrap().source_path,
            working_dir.join(format!("dst/blobs/{}", package_d.hash()))
        );

        // Validate that a subpackage is using the right paths as well
        let copied_subpackage_c = PackageManifest::try_load_from(
            working_dir.join(format!("dst/subpackages/{}", package_c.hash())),
        )
        .unwrap();
        assert_eq!(
            copied_subpackage_c.blobs().get(1).unwrap().source_path,
            working_dir.join(format!("dst/blobs/{}", package_c.blobs().get(1).unwrap().merkle))
        );
        assert_eq!(
            copied_subpackage_c.subpackages().get(0).unwrap().manifest_path,
            working_dir.join(format!("dst/subpackages/{}", package_b.hash()))
        );

        let inputs_for_depfile: Vec<Utf8PathBuf> = inputs_for_depfile.into_iter().collect();
        let expected_inputs: Vec<Utf8PathBuf> = vec![
            "src/package_a/file_1.text",
            "src/package_a/meta.far",
            "src/package_a/package_manifest.json",
            "src/package_b/file_1.text",
            "src/package_b/file_2.text",
            "src/package_b/meta.far",
            "src/package_b/package_manifest.json",
            "src/package_c/file_1.text",
            "src/package_c/meta.far",
            "src/package_c/package_manifest.json",
            "src/package_d/meta.far",
            "src/package_d/package_manifest.json",
        ]
        .into_iter()
        .map(|path| working_dir.join(path))
        .collect();

        assert_eq!(inputs_for_depfile, expected_inputs);
    }

    #[test]
    fn test_catch_duplicates() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = Utf8PathBuf::try_from(temp_dir.path().to_owned()).unwrap();

        let src_dir = working_dir.join("src");
        let src_dir_2 = working_dir.join("src_2");

        let (package_a_path, _package_a) =
            make_package("package", &["file_1"], &[], &src_dir).unwrap();

        let (package_b_path, _package_b) =
            make_package("package", &["file_1", "file_2"], &[], &src_dir_2).unwrap();

        let dst_dir = working_dir.join("dst");
        let packages_dir = dst_dir.join("packages");
        let subpackages_dir = dst_dir.join("subpackages");
        let blobstore = dst_dir.join("blobs");

        let mut package_copier = PackageCopier::new(&packages_dir, &subpackages_dir, &blobstore);
        package_copier.add_package_from_manifest_path(&package_a_path).unwrap();
        let result = package_copier.add_package_from_manifest_path(&package_b_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_allow_identical_duplicates() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = Utf8PathBuf::try_from(temp_dir.path().to_owned()).unwrap();

        let src_dir = working_dir.join("src");
        let src_dir_2 = working_dir.join("src_2");

        let (package_a_path, _package_a) =
            make_package("package", &["file_1"], &[], &src_dir).unwrap();

        let (package_b_path, _package_b) =
            make_package("package", &["file_1"], &[], &src_dir_2).unwrap();

        let dst_dir = working_dir.join("dst");
        let packages_dir = dst_dir.join("packages");
        let subpackages_dir = dst_dir.join("subpackages");
        let blobstore = dst_dir.join("blobs");

        let mut package_copier = PackageCopier::new(&packages_dir, &subpackages_dir, &blobstore);
        package_copier.add_package_from_manifest_path(&package_a_path).unwrap();
        let result = package_copier.add_package_from_manifest_path(&package_b_path);

        assert!(result.is_ok());
    }
}
