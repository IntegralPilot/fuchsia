// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_merkle::Hash;
use fuchsia_pkg::{PackageBuilder, PackageManifest, RelativeTo, SubpackageInfo};
use std::collections::{BTreeMap, BTreeSet};

/// A builder that constructs base packages.
#[derive(Default)]
pub struct SubpackageBlobsPackageBuilder {
    /// Maps the blob destination -> source. The destination is the blob's hash.
    contents: BTreeMap<Utf8PathBuf, Utf8PathBuf>,

    /// Tracks if we've processed a subpackage.
    seen: BTreeSet<Hash>,
}

impl SubpackageBlobsPackageBuilder {
    /// Add all the blobs from `package` into the subpackage blobs package being built.
    pub fn add_subpackages_from_package(&mut self, package: PackageManifest) -> Result<()> {
        let (_, subpackages) = package.into_blobs_and_subpackages();

        for subpackage in subpackages {
            self.add_files_from_subpackage(subpackage)?;
        }

        Ok(())
    }

    /// Recursively adds all the blobs from each subpackage to `contents`.
    fn add_files_from_subpackage(&mut self, subpackage: SubpackageInfo) -> Result<()> {
        // Exit early if we've already processed this subpackage. We will wait
        // until the very end of the function before we mark it as seen in case
        // we encounter any errors along the way.
        if self.seen.contains(&subpackage.merkle) {
            return Ok(());
        }

        let (blobs, child_subpackages) =
            PackageManifest::try_load_from(&subpackage.manifest_path)?.into_blobs_and_subpackages();

        for blob in blobs {
            self.contents
                .insert(Utf8PathBuf::from(blob.merkle.to_string()), blob.source_path.into());
        }

        for child_subpackage in child_subpackages {
            self.add_files_from_subpackage(child_subpackage)?;
        }

        self.seen.insert(subpackage.merkle);

        Ok(())
    }

    /// Build the base package and write the bytes to `out`.
    ///
    /// Intermediate files will be written to the directory specified by
    /// `gendir`.
    pub fn build(
        self,
        outdir: impl AsRef<Utf8Path>,
        gendir: impl AsRef<Utf8Path>,
        name: impl AsRef<str>,
        out: impl AsRef<Utf8Path>,
    ) -> Result<SubpackageBlobsPackageBuildResults> {
        let outdir = outdir.as_ref();
        let gendir = gendir.as_ref();
        let out = out.as_ref();

        // Write all generated files in a subdir with the name of the package.
        let gendir = gendir.join("subpackage_blobs");

        let Self { contents, seen: _ } = self;

        // It's not totally clear what the ABI revision means for the
        // subpackage_blobs package. It isn't checked anywhere. Since it's only
        // used as a vessel to transfer blobs, it's not clear who _would_ check
        // it.
        //
        // Set it to `INVALID` and decide on a more appropriate ABI revision
        // if/when we decide to check it.
        //
        // TODO(https://fxbug.dev/328787524): Clarify what this means.
        let mut builder =
            PackageBuilder::new("subpackage_blobs", version_history::AbiRevision::INVALID);
        // However, they can have different published names.  And the name here
        // is the name to publish it under (and to include in the generated
        // package manifest).
        builder.published_name(name);

        for (destination, source) in &contents {
            builder.add_file_as_blob(destination, source)?;
        }
        let manifest_path = outdir.join("package_manifest.json");
        builder.manifest_path(manifest_path.clone());
        builder.manifest_blobs_relative_to(RelativeTo::File);
        let manifest = builder.build(gendir.as_std_path(), out.as_std_path())?;

        Ok(SubpackageBlobsPackageBuildResults { contents, manifest, manifest_path })
    }
}

/// The results of building the `subpackage_blobs` package.
///
/// These are based on the information that the builder is configured with, and
/// then augmented with the operations that the `SubpackageBlobsPackageBuilder::build()`
/// fn performs, including an extra additions or removals.
///
/// This provides an audit trail of "what was created".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubpackageBlobsPackageBuildResults {
    /// Maps the blob destination -> source. The destination is the blob's hash.
    pub contents: BTreeMap<Utf8PathBuf, Utf8PathBuf>,
    pub manifest: PackageManifest,
    pub manifest_path: Utf8PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_archive::Utf8Reader;
    use pretty_assertions::assert_eq;
    use std::fs::File;
    use tempfile::TempDir;

    fn create_package(
        root: &Utf8Path,
        name: &str,
        subpackages: &[(&PackageManifest, &Utf8Path)],
    ) -> (PackageManifest, Utf8PathBuf) {
        let dir = root.join(name);

        // Hardcode the ABI so it doesn't change when the ABI revision is bumped.
        let mut builder = PackageBuilder::new(name, 0x57904F5A17FA3B22.into());

        let blob_name = format!("{}-blob", name);
        builder.add_contents_as_blob(&blob_name, &blob_name, &dir).unwrap();

        let meta_name = format!("meta/{}-meta", name);
        builder.add_contents_to_far(&meta_name, &meta_name, &dir).unwrap();

        for (subpackage_manifest, subpackage_path) in subpackages {
            builder
                .add_subpackage(
                    &subpackage_manifest.name().clone().into(),
                    subpackage_manifest.hash(),
                    subpackage_path.into(),
                )
                .unwrap();
        }

        let manifest = builder.build(&dir, dir.join("meta.far")).unwrap();

        let manifest_path = dir.join("package_manifest.json");
        serde_json::to_writer(File::create(&manifest_path).unwrap(), &manifest).unwrap();

        (manifest, manifest_path)
    }

    #[test]
    fn build_empty() {
        let builder = SubpackageBlobsPackageBuilder::default();

        let outdir_tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(outdir_tmp.path()).unwrap();

        let gendir_tmp = TempDir::new().unwrap();
        let gendir = Utf8Path::from_path(gendir_tmp.path()).unwrap();

        let far_path = outdir.join("subpackage_blobs.far");
        let build_results = builder.build(&outdir, &gendir, "subpackage_blobs", &far_path).unwrap();

        assert_eq!(build_results.contents, BTreeMap::new());
    }

    #[test]
    fn build_with_subpackages() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        // Create a few nested subpackages DAG to make sure we include all the
        // dependencies:
        //
        //  A    B   C
        //   \ /  \ /
        //    D    E    G
        //     \  /     |
        //      F       H     I

        let mut builder = SubpackageBlobsPackageBuilder::default();

        let (a_manifest, a_manifest_path) = create_package(&root, "a", &[]);
        let (b_manifest, b_manifest_path) = create_package(&root, "b", &[]);
        let (c_manifest, c_manifest_path) = create_package(&root, "c", &[]);

        let (d_manifest, d_manifest_path) = create_package(
            &root,
            "d",
            &[(&a_manifest, &a_manifest_path), (&b_manifest, &b_manifest_path)],
        );

        let (e_manifest, e_manifest_path) = create_package(
            &root,
            "e",
            &[(&b_manifest, &b_manifest_path), (&c_manifest, &c_manifest_path)],
        );

        let (f_manifest, _f_manifest_path) = create_package(
            &root,
            "f",
            &[(&d_manifest, &d_manifest_path), (&e_manifest, &e_manifest_path)],
        );
        builder.add_subpackages_from_package(f_manifest.clone()).unwrap();

        let (g_manifest, g_manifest_path) = create_package(&root, "g", &[]);

        let (h_manifest, _h_manifest_path) =
            create_package(&root, "h", &[(&g_manifest, &g_manifest_path)]);
        builder.add_subpackages_from_package(h_manifest.clone()).unwrap();

        let (i_manifest, _i_manifest_path) = create_package(&root, "i", &[]);
        builder.add_subpackages_from_package(i_manifest.clone()).unwrap();

        // Build the subpackage blobs package.
        let subpackage_blobs_dir = root.join("subpackage_blobs");
        std::fs::create_dir(&subpackage_blobs_dir).unwrap();

        let gendir_tmp = TempDir::new().unwrap();
        let gendir = Utf8Path::from_path(gendir_tmp.path()).unwrap();

        let far_path = root.join("subpackage_blobs.far");

        let build_results =
            builder.build(&subpackage_blobs_dir, &gendir, "subpackage_blobs", &far_path).unwrap();

        // Collect all the expected paths from the subpackages. This should not
        // include `f`, `h`, or `i`.
        let expected_merkles = a_manifest
            .blobs()
            .iter()
            .chain(b_manifest.blobs().iter())
            .chain(c_manifest.blobs().iter())
            .chain(d_manifest.blobs().iter())
            .chain(e_manifest.blobs().iter())
            .chain(g_manifest.blobs().iter())
            .map(|blob| (blob.merkle.to_string().into(), blob.source_path.clone().into()))
            .collect::<BTreeMap<_, _>>();

        // Validate that the contents are what we expected.
        assert_eq!(expected_merkles, build_results.contents);

        // Read the output and ensure it contains the right files (and their
        // hashes).
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(br#"{"name":"subpackage_blobs","version":"0"}"#, &*package);
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();

        let mut expected_contents = String::new();
        for merkle in expected_merkles.keys() {
            expected_contents.push_str(&format!("{}={}\n", merkle, merkle));
        }
        assert_eq!(contents, expected_contents);
    }
}
