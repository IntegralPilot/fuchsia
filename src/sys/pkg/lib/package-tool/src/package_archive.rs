// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{
    PackageArchiveAddCommand, PackageArchiveCreateCommand, PackageArchiveEditCommand,
    PackageArchiveExtractCommand, PackageArchiveRemoveCommand,
};
use crate::{BLOBS_JSON_NAME, PACKAGE_MANIFEST_NAME, to_writer_json_pretty, write_depfile};
use anyhow::{Context as _, Result, anyhow};
use buf_read_ext::BufReadExt as _;
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_archive as far;
use fuchsia_pkg::{PackageBuilder, PackageManifest, SubpackageInfo};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub async fn cmd_package_archive_create(cmd: PackageArchiveCreateCommand) -> Result<()> {
    let package_manifest = PackageManifest::try_load_from(&cmd.package_manifest)
        .with_context(|| format!("opening {}", cmd.package_manifest))?;

    if let Some(parent) = cmd.out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let output = File::create(&cmd.out)
        .with_context(|| format!("creating package archive file {}", cmd.out.display()))?;

    package_manifest
        .clone()
        .archive(cmd.root_dir, output)
        .await
        .with_context(|| format!("writing archive {}", cmd.out.display()))?;

    if let Some(depfile_path) = &cmd.depfile {
        let mut dep_paths: BTreeSet<String> = BTreeSet::<String>::new();

        // Getting top-level blobs
        for blob in package_manifest.blobs() {
            dep_paths.insert(blob.source_path.clone());
        }

        // Recursively checking each Subpackage, building `dep_paths` as we go.
        let mut visited_subpackages: BTreeSet<fuchsia_merkle::Hash> =
            BTreeSet::<fuchsia_merkle::Hash>::new();
        let mut subpackage_list: Vec<SubpackageInfo> = package_manifest.subpackages().to_vec();

        while let Some(subpackage) = subpackage_list.pop() {
            // If subpackage already seen, skip.
            if visited_subpackages.contains(&subpackage.merkle) {
                continue;
            }
            visited_subpackages.insert(subpackage.merkle);

            dep_paths.insert(subpackage.manifest_path.clone());

            let subpackage_manifest = PackageManifest::try_load_from(&subpackage.manifest_path)
                .with_context(|| format!("opening {}", subpackage.manifest_path))?;

            // Gathering subpackage blobs.
            for subpackage_blob in subpackage_manifest.blobs() {
                dep_paths.insert(subpackage_blob.source_path.clone());
            }

            // Gathering possible additional subpackages.
            for inner_subpackage in subpackage_manifest.subpackages() {
                subpackage_list.push(inner_subpackage.clone());
            }
        }

        let far_path = Utf8Path::from_path(cmd.out.as_path()).unwrap();

        write_depfile(depfile_path.as_std_path(), far_path, dep_paths.into_iter())?;
    }

    Ok(())
}

pub async fn cmd_package_archive_extract(cmd: PackageArchiveExtractCommand) -> Result<()> {
    let blobs_dir = cmd.out.join("blobs");

    std::fs::create_dir_all(&blobs_dir)
        .with_context(|| format!("creating directory {blobs_dir}"))?;

    let manifests_dir = cmd.out.join("manifests");

    std::fs::create_dir_all(&manifests_dir)
        .with_context(|| format!("creating directory {manifests_dir}"))?;

    let mut package_manifest = PackageManifest::from_archive(
        &cmd.archive,
        blobs_dir.as_std_path(),
        manifests_dir.as_std_path(),
    )
    .with_context(|| format!("extracting package manifest {}", cmd.archive.display()))?;

    if let Some(repository) = cmd.repository {
        package_manifest.set_repository(Some(repository));
    }

    let package_manifest_path = cmd.out.join(PACKAGE_MANIFEST_NAME);
    let package_manifest = package_manifest
        .write_with_relative_paths(&package_manifest_path)
        .with_context(|| format!("creating {package_manifest_path}"))?;

    // TODO(https://fxbug.dev/300181454): blobs.json files are deprecated. Support to output
    // blobs.json will be removed once migration to using package_manifest.json is completed.
    if cmd.blobs_json {
        let blobs_json_path = cmd.out.join(BLOBS_JSON_NAME);
        let file = File::create(&blobs_json_path)
            .with_context(|| format!("creating {blobs_json_path}"))?;
        to_writer_json_pretty(file, package_manifest.blobs())?;
    }

    if cmd.namespace { populate_namespace(cmd.archive, cmd.out.into()).await } else { Ok(()) }
}

pub async fn cmd_package_archive_add(cmd: PackageArchiveAddCommand) -> Result<()> {
    package_archive_modify(&cmd.archive, &cmd.output, |pkg_builder| {
        pkg_builder.overwrite_files(cmd.overwrite);

        let path_of_file_in_archive = cmd
            .path_of_file_in_archive
            .to_str()
            .ok_or_else(|| anyhow!("couldn't create str from archive file path"))?;
        let file_to_add = std::fs::canonicalize(&cmd.file_to_add)
            .with_context(|| format!("canonicalizing {}", cmd.file_to_add.display()))?;
        let file_to_add =
            file_to_add.to_str().ok_or_else(|| anyhow!("couldn't create str from file path"))?;

        if cmd.path_of_file_in_archive.starts_with("meta/") {
            pkg_builder.add_file_to_far(path_of_file_in_archive, file_to_add)?;
        } else {
            pkg_builder.add_file_as_blob(path_of_file_in_archive, file_to_add)?;
        }

        Ok(())
    })
    .await
}

pub async fn cmd_package_archive_remove(cmd: PackageArchiveRemoveCommand) -> Result<()> {
    package_archive_modify(&cmd.archive, &cmd.output, |pkg_builder| {
        let file_to_remove = cmd
            .file_to_remove
            .to_str()
            .ok_or_else(|| anyhow!("couldn't create str from file path"))?;

        if cmd.file_to_remove.starts_with("meta/") {
            pkg_builder.remove_file_from_far(file_to_remove)?;
        } else {
            pkg_builder.remove_blob_file(file_to_remove)?;
        }
        Ok(())
    })
    .await
}

pub async fn cmd_package_archive_edit(cmd: PackageArchiveEditCommand) -> Result<()> {
    package_archive_modify(&cmd.archive, &cmd.output, |pkg_builder| {
        if let Some(new_name) = cmd.package_name {
            pkg_builder.name(new_name);
        }

        if let Some(abi) = cmd.unsafe_override_abi_revision {
            eprintln!(
                "\
WARNING: Overriding package ABI revision. Package was built for ABI revision
0x{}, but now claims to be compatible with ABI revision 0x{}.
Devices that support ABI revision 0x{} may attempt to run components in
this package and experience compatibility issues!",
                pkg_builder.abi_revision, abi, abi
            );
            pkg_builder.abi_revision = abi;
        }

        Ok(())
    })
    .await
}

/// Reads an archive, creates a PackageBuilder, runs a callback on it, and
/// serializes the PackageBuilder.
async fn package_archive_modify(
    archive: &Path,
    output: &Path,
    f: impl FnOnce(&mut PackageBuilder) -> Result<()>,
) -> Result<()> {
    // Extract the archive
    let tmp = TempDir::new()?;
    let root =
        Utf8Path::from_path(tmp.path()).ok_or_else(|| anyhow!("couldn't create utf-8 path"))?;
    let extract_dir = root.join("extract");

    cmd_package_archive_extract(PackageArchiveExtractCommand {
        out: extract_dir.clone(),
        repository: None,
        blobs_json: true,
        namespace: false,
        archive: archive.into(),
    })
    .await?;

    // Add the file, either in the meta.far or the content blobs
    let pkg_builder_outdir = TempDir::new()?;
    let original_pkg_manifest =
        PackageManifest::try_load_from(extract_dir.join(PACKAGE_MANIFEST_NAME))?;
    let mut pkg_builder =
        PackageBuilder::from_manifest(original_pkg_manifest, pkg_builder_outdir.path())?;

    f(&mut pkg_builder)?;

    // Serialize the archive to the output path
    let build_tmpdir = TempDir::new()?;
    let new_pkg_manifest =
        pkg_builder.build(build_tmpdir.path(), build_tmpdir.path().join("meta.far"))?;
    let new_archive = File::create(output)
        .with_context(|| format!("creating new package archive file {}", output.display()))?;
    let () = new_pkg_manifest.archive(extract_dir, new_archive).await?;

    Ok(())
}

async fn populate_namespace(far_file: PathBuf, output_dir: PathBuf) -> Result<()> {
    let file = File::open(&far_file)
        .with_context(|| format!("failed to open file: {}", far_file.display()))?;
    let mut reader = far::Utf8Reader::new(file)
        .with_context(|| format!("failed to parse FAR file: {}", far_file.display()))?;
    let blobs_dir = output_dir.join("blobs");
    let output_dir = output_dir.join("pkg");

    #[allow(clippy::needless_collect)] // avoids conflicting immutable & mutable borrows
    let far_content_paths: Vec<_> = reader.list().map(|e| e.path()).collect();
    if far_content_paths.contains(&"meta.far") {
        // Extract the meta.far from the package
        let mut path_map = HashMap::new();
        path_map.insert("meta.far".to_owned(), "meta.far".to_owned());
        extract_far_to_dir(&mut reader, &output_dir, &blobs_dir, Some(path_map)).await?;
        // Extract the meta.far itself
        let meta_far_file =
            File::open(output_dir.join("meta.far")).with_context(|| "failed to open meta.far")?;
        let mut meta_far_reader =
            far::Utf8Reader::new(meta_far_file).with_context(|| "failed to parse meta.far")?;
        extract_far_to_dir(&mut meta_far_reader, &output_dir, &blobs_dir, None).await?;
        // Remove the meta.far now that it is extracted
        fs::remove_file(output_dir.join("meta.far"))
            .with_context(|| "failed to remove the temporary meta.far")?;
        // Extract the hash-to-path mapping
        let hash_path_map =
            extract_mapping_from_contents(&output_dir.join("meta").join("contents")).await?;
        // Finally, extract the files respecting the mapping from the contents file
        extract_far_to_dir(&mut reader, &output_dir, &blobs_dir, Some(hash_path_map)).await?;
    } else {
        log::info!("No meta.far included, simply extracting all files.");
        extract_far_to_dir(&mut reader, &output_dir, &blobs_dir, None).await?;
    }

    Ok(())
}

async fn extract_far_to_dir(
    reader: &mut far::Utf8Reader<File>,
    output_dir: &Path,
    blobs_dir: &Path,
    path_map: Option<HashMap<String, String>>,
) -> Result<()> {
    let src_paths: Vec<Utf8PathBuf> = if let Some(ref m) = path_map {
        m.keys().map(Utf8PathBuf::from).collect()
    } else {
        reader.list().map(|e| Utf8PathBuf::from(e.path())).collect()
    };
    for src_path in src_paths {
        // dst_path is resolved from the hash map if an entry exists
        let dst_path = if let Some(ref m) = path_map {
            if let Some(p) = m.get(src_path.as_str()) {
                output_dir.join(p)
            } else {
                unreachable!();
            }
        } else {
            output_dir.join(&src_path)
        };

        let parent = dst_path.parent().expect("`path` must be non-empty");
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;

        // We can link the blobs from the /blobs dir which was populated during archive extract
        if std::fs::hard_link(blobs_dir.join(src_path.as_str()), &dst_path).is_err() {
            // The files that do not exist in /blobs (e.g. meta.far and its content) will return
            // an error when trying to link. We then extract them directly from the far file.
            let bytes = reader
                .read_file(src_path.as_str())
                .with_context(|| format!("failed to read {src_path} from FAR"))?;
            fs::write(&dst_path, &bytes)
                .with_context(|| format!("failed to write {}", dst_path.display()))?;
        }
    }
    Ok(())
}

async fn extract_mapping_from_contents(path: &PathBuf) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let contents_file = File::open(path).context("failed to open meta/contents")?;
    let mut reader = BufReader::new(contents_file);
    let mut lines = reader.lending_lines();
    while let Some(line) = lines.next() {
        let line = line.context("failed to read line from meta/contents")?;
        let m: Vec<_> = line.split("=").collect();
        if m.len() != 2 {
            return Err(anyhow!("meta/contents contains invalid entry: {line}"));
        } else {
            if m[0].is_empty() || m[1].is_empty() {
                return Err(anyhow!("meta/contents contains invalid entry: {line}"));
            }
            map.insert(m[1].to_owned(), m[0].to_owned());
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert_to_depfile_filepath;
    use camino::Utf8PathBuf;
    use fuchsia_archive::Utf8Reader;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use version_history::AbiRevision;

    const BIN_CONTENTS: &[u8] = b"bin";
    const LIB_CONTENTS: &[u8] = b"lib";

    const ORIGINAL_ABI_REVISION: AbiRevision = AbiRevision::from_u64(0x406C7CA7EF077DB4);
    const DIFFERENT_ABI_REVISION: AbiRevision = AbiRevision::from_u64(0x410224d48526d927);
    const META_FAR_HASH: &str = "f1a91cbbd41fef65416522a9de7e1d8be0f962ec6371cb747a403cff03d656e6";
    const META_FAR_HASH_WITH_ADDED_FILE: &str =
        "c1f005cbf6e7d71cf1014c2ab8a493b55640ef169aa2b94f211cd0588236f989";
    const META_FAR_HASH_WITH_MODIFIED_BIN: &str =
        "9084b9e928d29b97e39a1f8f155c7e1ec1aa005cf43fe9a6b958b160d3741a3e";
    const META_FAR_HASH_WITHOUT_BIN: &str =
        "2c8dfa9b2b2b095109ca1e37edb6cbbe16cf474bf4b523247b9aac8a5b66fcac";
    const META_FAR_HASH_WITH_DIFFERENT_NAME: &str =
        "5db1fe0f954a7df31b569b0dd3c6c830a4a8f6b77bba6731347f6b9adfe8bc0f";
    const META_FAR_HASH_WITH_DIFFERENT_ABI_REVISION: &str =
        "5840c40916b502e1c04ccba36a17329589d4c9b6e0912da3aa63de70ba77d102";
    const BIN_HASH: &str = "5d202ed772f4de29ecd7bc9a3f20278cd69ae160e36ba8b434512ca45003c7a3";
    const MODIFIED_BIN_HASH: &str =
        "8b9dd6886ff377a19d8716a30a0659897fba5cbdfb43649bf93317fcb6fdb18c";
    const LIB_HASH: &str = "65f1e8f09fdc18cbcfc8f2a472480643761478595e891138de8055442dcc3233";
    const ADDED_FILE_HASH: &str =
        "8b9dd6886ff377a19d8716a30a0659897fba5cbdfb43649bf93317fcb6fdb18c";

    struct Package {
        manifest_path: Utf8PathBuf,
        meta_far_contents: Vec<u8>,
    }

    fn create_package(pkg_dir: &Utf8Path) -> Package {
        let mut builder = PackageBuilder::new("some_pkg_name", ORIGINAL_ABI_REVISION);
        builder.add_contents_as_blob("bin", BIN_CONTENTS, pkg_dir).unwrap();
        builder.add_contents_as_blob("lib", LIB_CONTENTS, pkg_dir).unwrap();

        // Build the package.
        let metafar_path = pkg_dir.join("meta.far");
        let manifest = builder.build(pkg_dir, &metafar_path).unwrap();
        let meta_far_contents = std::fs::read(&metafar_path).unwrap();

        let manifest_path = pkg_dir.join(PACKAGE_MANIFEST_NAME);

        serde_json::to_writer(std::fs::File::create(&manifest_path).unwrap(), &manifest).unwrap();

        Package { manifest_path, meta_far_contents }
    }

    fn read_archive(archive: &mut Utf8Reader<std::fs::File>) -> BTreeMap<String, Vec<u8>> {
        archive
            .list()
            .map(|e| e.path().to_owned())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|path| {
                let contents = archive.read_file(&path).unwrap();
                (path, contents)
            })
            .collect()
    }

    fn read_dir(dir: &Utf8Path) -> BTreeMap<Utf8PathBuf, Vec<u8>> {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                if entry.metadata().unwrap().is_file() {
                    let path = Utf8Path::from_path(entry.path()).unwrap().into();
                    let contents = std::fs::read(entry.path()).unwrap();
                    Some((path, contents))
                } else {
                    None
                }
            })
            .collect()
    }

    fn check_extract(
        package: &Package,
        extract_dir: &Utf8Path,
        blob_sources_relative: Option<&str>,
        mut meta_far_path: Utf8PathBuf,
        mut bin_path: Utf8PathBuf,
        mut lib_path: Utf8PathBuf,
    ) -> BTreeMap<Utf8PathBuf, Vec<u8>> {
        let manifest_path = extract_dir.join("package_manifest.json");

        // Read the extracted files.
        let mut extract_contents = read_dir(extract_dir);

        // Check the extracted manifest is correct.
        let mut expected = serde_json::json!({
            "version": "1",
            "package": {
                "name": "some_pkg_name",
                "version": "0"
            },
            "abi_revision": "0x406c7ca7ef077db4",
            "blobs": [
                {
                    "merkle": META_FAR_HASH,
                    "path": "meta/",
                    "size": 16384,
                    "source_path": meta_far_path,
                },
                {
                    "merkle": BIN_HASH,
                    "path": "bin",
                    "size": 3,
                    "source_path": bin_path,
                },
                {
                    "merkle": LIB_HASH,
                    "path": "lib",
                    "size": 3,
                    "source_path": lib_path,
                },
            ]
        });

        if let Some(blob_sources_relative) = blob_sources_relative {
            expected
                .as_object_mut()
                .unwrap()
                .insert("blob_sources_relative".into(), blob_sources_relative.into());

            if blob_sources_relative == "file" {
                meta_far_path = extract_dir.join(meta_far_path);
                bin_path = extract_dir.join(bin_path);
                lib_path = extract_dir.join(lib_path);
            }
        }

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&manifest_path).unwrap()
            )
            .unwrap(),
            expected,
        );

        // Check that the meta.far is right.
        assert_eq!(extract_contents.remove(&meta_far_path).unwrap(), package.meta_far_contents);

        // Check the rest of the blobs are correct.
        assert_eq!(extract_contents.remove(&bin_path).unwrap(), BIN_CONTENTS);
        assert_eq!(extract_contents.remove(&lib_path).unwrap(), LIB_CONTENTS);

        extract_contents
    }

    #[fuchsia::test]
    async fn test_archive_create_and_extract() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: None,
        })
        .await
        .unwrap();

        // Read the generated archive file.
        let mut archive = Utf8Reader::new(File::open(&archive_path).unwrap()).unwrap();

        assert_eq!(
            read_archive(&mut archive),
            BTreeMap::from([
                ("meta.far".to_string(), package.meta_far_contents.clone()),
                (BIN_HASH.to_string(), BIN_CONTENTS.to_vec()),
                (LIB_HASH.to_string(), LIB_CONTENTS.to_vec()),
            ]),
        );

        // Extract the archive.
        let extract_dir = root.join("extract");
        cmd_package_archive_extract(PackageArchiveExtractCommand {
            out: extract_dir.clone(),
            repository: None,
            archive: archive_path.clone().into(),
            blobs_json: true,
            namespace: false,
        })
        .await
        .unwrap();

        let mut extract_contents = check_extract(
            &package,
            &extract_dir,
            Some("file"),
            Utf8Path::new("blobs").join(META_FAR_HASH),
            Utf8Path::new("blobs").join(BIN_HASH),
            Utf8Path::new("blobs").join(LIB_HASH),
        );

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                    {
                        "source_path": format!("blobs/{META_FAR_HASH}"),
                        "path": "meta/",
                        "merkle": META_FAR_HASH,
                        "size": 16384,
                    },
                    {
                        "source_path": format!("blobs/{BIN_HASH}"),
                        "path": "bin",
                        "merkle": BIN_HASH,
                        "size": 3,
                    },
                    {
                        "source_path": format!("blobs/{LIB_HASH}"),
                        "path": "lib",
                        "merkle": LIB_HASH,
                        "size": 3,
                    },
                ]
            )
        );

        assert_eq!(extract_contents, BTreeMap::new());
    }

    /// Returns the path of the directory into which we extracted the modified far
    async fn test_archive_add_inner(
        tmp: &TempDir,
        path_to_add: Utf8PathBuf,
        contents_to_add: &str,
        overwrite: bool,
    ) -> Result<Utf8PathBuf, anyhow::Error> {
        //let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        let host_path_to_add = root.join(path_to_add.clone());

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: None,
        })
        .await
        .unwrap();

        // Read the generated archive file.
        let mut archive = Utf8Reader::new(File::open(&archive_path).unwrap()).unwrap();

        assert_eq!(
            read_archive(&mut archive),
            BTreeMap::from([
                ("meta.far".to_string(), package.meta_far_contents.clone()),
                (BIN_HASH.to_string(), BIN_CONTENTS.to_vec()),
                (LIB_HASH.to_string(), LIB_CONTENTS.to_vec()),
            ]),
        );

        // Add a file to the archive
        std::fs::write(&host_path_to_add, contents_to_add)
            .context(format!("writing contents to file: {host_path_to_add}"))
            .unwrap();

        cmd_package_archive_add(PackageArchiveAddCommand {
            archive: archive_path.clone().into(),
            file_to_add: host_path_to_add.clone().into(),
            path_of_file_in_archive: path_to_add.clone().into(),
            output: archive_path.clone().into(),
            overwrite,
        })
        .await?;

        // Extract the archive.
        let extract_dir = root.join("extract");
        cmd_package_archive_extract(PackageArchiveExtractCommand {
            out: extract_dir.clone(),
            repository: None,
            archive: archive_path.clone().into(),
            blobs_json: true,
            namespace: false,
        })
        .await
        .unwrap();

        Ok(extract_dir)
    }

    #[fuchsia::test]
    async fn test_archive_add() {
        let tmp = TempDir::new().unwrap();
        let extract_dir =
            test_archive_add_inner(&tmp, "add_test".into(), "test", false).await.unwrap();

        let mut extract_contents = read_dir(&extract_dir);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                    {
                        "source_path": format!("blobs/{META_FAR_HASH_WITH_ADDED_FILE}"),
                        "path": "meta/",
                        "merkle": META_FAR_HASH_WITH_ADDED_FILE,
                        "size": 16384,
                    },
                    {
                        "source_path": format!("blobs/{ADDED_FILE_HASH}"),
                        "path": "add_test",
                        "merkle": ADDED_FILE_HASH,
                        "size": 4,
                    },
                    {
                        "source_path": format!("blobs/{BIN_HASH}"),
                        "path": "bin",
                        "merkle": BIN_HASH,
                        "size": 3,
                    },
                    {
                        "source_path": format!("blobs/{LIB_HASH}"),
                        "path": "lib",
                        "merkle": LIB_HASH,
                        "size": 3,
                    },
                ]
            )
        );
    }

    #[fuchsia::test]
    async fn test_archive_add_overwrite_fails_when_flag_not_set() {
        let tmp = TempDir::new().unwrap();
        let res = test_archive_add_inner(&tmp, "bin".into(), "test", false).await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Package 'some_pkg_name' already contains a file (as a blob) at: 'bin'"
        );
    }

    #[fuchsia::test]
    async fn test_archive_add_overwrite_succeeds_when_flag_set() {
        let tmp = TempDir::new().unwrap();
        let extract_dir = test_archive_add_inner(&tmp, "bin".into(), "test", true).await.unwrap();

        let mut extract_contents = read_dir(&extract_dir);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                    {
                        "source_path": format!("blobs/{META_FAR_HASH_WITH_MODIFIED_BIN}"),
                        "path": "meta/",
                        "merkle": META_FAR_HASH_WITH_MODIFIED_BIN,
                        "size": 16384,
                    },
                    {
                        "source_path": format!("blobs/{MODIFIED_BIN_HASH}"),
                        "path": "bin",
                        "merkle": MODIFIED_BIN_HASH,
                        "size": 4,
                    },
                    {
                        "source_path": format!("blobs/{LIB_HASH}"),
                        "path": "lib",
                        "merkle": LIB_HASH,
                        "size": 3,
                    },
                ]
            )
        );
    }

    #[fuchsia::test]
    async fn test_archive_remove() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: None,
        })
        .await
        .unwrap();

        // Read the generated archive file.
        let mut archive = Utf8Reader::new(File::open(&archive_path).unwrap()).unwrap();

        assert_eq!(
            read_archive(&mut archive),
            BTreeMap::from([
                ("meta.far".to_string(), package.meta_far_contents.clone()),
                (BIN_HASH.to_string(), BIN_CONTENTS.to_vec()),
                (LIB_HASH.to_string(), LIB_CONTENTS.to_vec()),
            ]),
        );

        // Remove a file from the archive
        cmd_package_archive_remove(PackageArchiveRemoveCommand {
            archive: archive_path.clone().into(),
            file_to_remove: "bin".into(),
            output: archive_path.clone().into(),
        })
        .await
        .unwrap();

        // Extract the archive.
        let extract_dir = root.join("extract");
        cmd_package_archive_extract(PackageArchiveExtractCommand {
            out: extract_dir.clone(),
            repository: None,
            archive: archive_path.clone().into(),
            blobs_json: true,
            namespace: false,
        })
        .await
        .unwrap();

        let mut extract_contents = read_dir(&extract_dir);

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                    {
                        "source_path": format!("blobs/{META_FAR_HASH_WITHOUT_BIN}"),
                        "path": "meta/",
                        "merkle": META_FAR_HASH_WITHOUT_BIN,
                        "size": 16384,
                    },
                    {
                        "source_path": format!("blobs/{LIB_HASH}"),
                        "path": "lib",
                        "merkle": LIB_HASH,
                        "size": 3,
                    },
                ]
            )
        );
    }

    #[fuchsia::test]
    async fn test_archive_edit_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: None,
        })
        .await
        .unwrap();

        // Read the generated archive file.
        let mut archive = Utf8Reader::new(File::open(&archive_path).unwrap()).unwrap();

        assert_eq!(
            read_archive(&mut archive),
            BTreeMap::from([
                ("meta.far".to_string(), package.meta_far_contents.clone()),
                (BIN_HASH.to_string(), BIN_CONTENTS.to_vec()),
                (LIB_HASH.to_string(), LIB_CONTENTS.to_vec()),
            ]),
        );

        // Null edit.
        cmd_package_archive_edit(PackageArchiveEditCommand {
            archive: archive_path.clone().into(),
            package_name: None,
            unsafe_override_abi_revision: None,
            output: archive_path.clone().into(),
        })
        .await
        .unwrap();

        // Extract the archive.
        let extract_dir = root.join("extract");
        cmd_package_archive_extract(PackageArchiveExtractCommand {
            out: extract_dir.clone(),
            repository: None,
            archive: archive_path.clone().into(),
            blobs_json: true,
            namespace: false,
        })
        .await
        .unwrap();

        let mut extract_contents = read_dir(&extract_dir);

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                {
                    "source_path": format!("blobs/{META_FAR_HASH}"),
                    "path": "meta/",
                    "merkle": META_FAR_HASH,
                    "size": 16384,
                },
                {
                    "source_path": format!("blobs/{BIN_HASH}"),
                    "path": "bin",
                    "merkle": BIN_HASH,
                    "size": 3,
                },
                {
                    "source_path": format!("blobs/{LIB_HASH}"),
                    "path": "lib",
                    "merkle": LIB_HASH,
                    "size": 3,
                },
            ])
        );
    }

    #[fuchsia::test]
    async fn test_archive_edit_name() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: None,
        })
        .await
        .unwrap();

        // Read the generated archive file.
        let mut archive = Utf8Reader::new(File::open(&archive_path).unwrap()).unwrap();

        assert_eq!(
            read_archive(&mut archive),
            BTreeMap::from([
                ("meta.far".to_string(), package.meta_far_contents.clone()),
                (BIN_HASH.to_string(), BIN_CONTENTS.to_vec()),
                (LIB_HASH.to_string(), LIB_CONTENTS.to_vec()),
            ]),
        );

        // Change the package name.
        cmd_package_archive_edit(PackageArchiveEditCommand {
            archive: archive_path.clone().into(),
            package_name: Some("new-package-name".to_string()),
            unsafe_override_abi_revision: None,
            output: archive_path.clone().into(),
        })
        .await
        .unwrap();

        // Extract the archive.
        let extract_dir = root.join("extract");
        cmd_package_archive_extract(PackageArchiveExtractCommand {
            out: extract_dir.clone(),
            repository: None,
            archive: archive_path.clone().into(),
            blobs_json: true,
            namespace: false,
        })
        .await
        .unwrap();

        let mut extract_contents = read_dir(&extract_dir);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                {
                    "source_path": format!("blobs/{META_FAR_HASH_WITH_DIFFERENT_NAME}"),
                    "path": "meta/",
                    "merkle": META_FAR_HASH_WITH_DIFFERENT_NAME,
                    "size": 16384,
                },
                {
                    "source_path": format!("blobs/{BIN_HASH}"),
                    "path": "bin",
                    "merkle": BIN_HASH,
                    "size": 3,
                },
                {
                    "source_path": format!("blobs/{LIB_HASH}"),
                    "path": "lib",
                    "merkle": LIB_HASH,
                    "size": 3,
                },
            ])
        );
    }

    #[fuchsia::test]
    async fn test_archive_edit_abi_revision() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: None,
        })
        .await
        .unwrap();

        // Change the ABI stamp.
        cmd_package_archive_edit(PackageArchiveEditCommand {
            archive: archive_path.clone().into(),
            package_name: None,
            unsafe_override_abi_revision: Some(DIFFERENT_ABI_REVISION),
            output: archive_path.clone().into(),
        })
        .await
        .unwrap();

        // Extract the archive.
        let extract_dir = root.join("extract");
        cmd_package_archive_extract(PackageArchiveExtractCommand {
            out: extract_dir.clone(),
            repository: None,
            archive: archive_path.clone().into(),
            blobs_json: true,
            namespace: false,
        })
        .await
        .unwrap();

        let mut extract_contents = read_dir(&extract_dir);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &extract_contents.remove(&extract_dir.join(BLOBS_JSON_NAME)).unwrap(),
            )
            .unwrap(),
            serde_json::json!([
                {
                    // NOTE(hjfreyer): I've manually verified that this has the
                    // correct ABI stamp in meta/fuchsia.abi/abi-revision.
                    "source_path": format!("blobs/{META_FAR_HASH_WITH_DIFFERENT_ABI_REVISION}"),
                    "path": "meta/",
                    "merkle": META_FAR_HASH_WITH_DIFFERENT_ABI_REVISION,
                    "size": 16384,
                },
                {
                    "source_path": format!("blobs/{BIN_HASH}"),
                    "path": "bin",
                    "merkle": BIN_HASH,
                    "size": 3,
                },
                {
                    "source_path": format!("blobs/{LIB_HASH}"),
                    "path": "lib",
                    "merkle": LIB_HASH,
                    "size": 3,
                },
            ])
        );
    }

    #[fuchsia::test]
    async fn test_archive_create_with_depfile() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();

        let pkg_dir = root.join("pkg");
        let package = create_package(&pkg_dir);
        let depfile_path = root.join("archive.far.d");

        // Write the archive.
        let archive_path = root.join("archive.far");
        cmd_package_archive_create(PackageArchiveCreateCommand {
            out: archive_path.clone().into(),
            root_dir: pkg_dir.to_owned(),
            package_manifest: package.manifest_path.clone(),
            depfile: Some(depfile_path.clone()),
        })
        .await
        .unwrap();

        // Read the generated archive file.
        let mut archive = Utf8Reader::new(File::open(&archive_path).unwrap()).unwrap();

        assert_eq!(
            read_archive(&mut archive),
            BTreeMap::from([
                ("meta.far".to_string(), package.meta_far_contents.clone()),
                (BIN_HASH.to_string(), BIN_CONTENTS.to_vec()),
                (LIB_HASH.to_string(), LIB_CONTENTS.to_vec()),
            ]),
        );

        let expected_deps = [
            convert_to_depfile_filepath(root.join("pkg/meta.far").as_str()),
            convert_to_depfile_filepath(root.join("pkg/bin").as_str()),
            convert_to_depfile_filepath(root.join("pkg/lib").as_str()),
        ];

        assert_eq!(
            std::fs::read_to_string(&depfile_path).unwrap(),
            format!(
                "{}: {}",
                convert_to_depfile_filepath(root.join("archive.far").as_str()),
                expected_deps
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        );
    }
}
