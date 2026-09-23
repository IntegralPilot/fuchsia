// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::PackageBuildCommand;
use crate::{BLOBS_JSON_NAME, PACKAGE_MANIFEST_NAME, to_writer_json_pretty, write_depfile};
use anyhow::{Context as _, Result};
use fuchsia_pkg::{
    PackageBuildManifest, PackageBuilder, SubpackagesBuildManifest, SubpackagesBuildManifestEntry,
    SubpackagesBuildManifestEntryKind,
};
use std::collections::BTreeSet;
use std::fs::{File, create_dir_all};
use std::io::{BufReader, BufWriter, Write};
use tempfile::NamedTempFile;
use tempfile_ext::NamedTempFileExt as _;

const META_FAR_NAME: &str = "meta.far";
const META_FAR_DEPFILE_NAME: &str = "meta.far.d";
const BLOBS_MANIFEST_NAME: &str = "blobs.manifest";

pub async fn cmd_package_build(cmd: PackageBuildCommand) -> Result<()> {
    cmd_package_build_with_history(cmd, version_history_data::HISTORY).await
}

pub async fn cmd_package_build_with_history(
    cmd: PackageBuildCommand,
    history: version_history::VersionHistory,
) -> Result<()> {
    let package_build_manifest = File::open(&cmd.package_build_manifest_path)
        .with_context(|| format!("opening {}", cmd.package_build_manifest_path))?;

    let package_build_manifest =
        PackageBuildManifest::from_pm_fini(BufReader::new(package_build_manifest))
            .with_context(|| format!("reading {}", cmd.package_build_manifest_path))?;

    let abi_revision = match history.check_api_level_for_build(cmd.api_level) {
        Ok(abi_revision) => abi_revision,
        Err(err) => return Err(err.into()),
    };

    let mut builder =
        PackageBuilder::from_package_build_manifest(&package_build_manifest, abi_revision)
            .with_context(|| {
                format!("creating package manifest from {}", cmd.package_build_manifest_path)
            })?;

    if let Some(published_name) = &cmd.published_name {
        builder.published_name(published_name);
    }

    if let Some(repository) = &cmd.repository {
        builder.repository(repository);
    }

    let subpackages_build_manifest =
        if let Some(subpackages_build_manifest_path) = &cmd.subpackages_build_manifest_path {
            let f = File::open(subpackages_build_manifest_path)?;
            Some(SubpackagesBuildManifest::deserialize(BufReader::new(f))?)
        } else {
            None
        };

    if let Some(subpackages_build_manifest) = &subpackages_build_manifest {
        for (url, hash, package_manifest_path) in subpackages_build_manifest.to_subpackages()? {
            builder
                .add_subpackage(&url, hash, package_manifest_path.into())
                .with_context(|| format!("adding subpackage {url} : {hash}"))?;
        }
    }

    if !cmd.out.exists() {
        create_dir_all(&cmd.out).with_context(|| format!("creating {}", cmd.out))?;
    }

    let package_manifest_path = cmd.out.join(PACKAGE_MANIFEST_NAME);
    builder.manifest_path(package_manifest_path);

    // Build the package.
    let gendir = tempfile::TempDir::new_in(&cmd.out)?;
    let meta_far_path = cmd.out.join(META_FAR_NAME);
    let package_manifest = builder
        .build(gendir.path(), &meta_far_path)
        .with_context(|| format!("creating package manifest {meta_far_path}"))?;

    // FIXME(https://fxbug.dev/42052117): We're replicating `pm build --depfile` here, and directly expressing
    // that the `meta.far` depends on all the package contents. However, I think this should
    // ultimately be unnecessary, since the build systems should be separately tracking that the
    // creation manifest already depends on the package contents. We should make sure all build
    // systems support this, and remove the `--depfile` here.
    if cmd.depfile {
        let mut deps = package_build_manifest
            .far_contents()
            .values()
            .chain(package_build_manifest.external_contents().values())
            .map(|s| s.as_str())
            .collect::<BTreeSet<_>>();

        if let Some(subpackages_build_manifest_path) = &cmd.subpackages_build_manifest_path {
            deps.insert(subpackages_build_manifest_path.as_str());
        }

        if let Some(subpackages_build_manifest) = &subpackages_build_manifest {
            for entry in subpackages_build_manifest.entries() {
                let SubpackagesBuildManifestEntry { kind, package_manifest_file } = entry;
                match kind {
                    SubpackagesBuildManifestEntryKind::Empty => {}
                    SubpackagesBuildManifestEntryKind::Url(_) => {}
                    SubpackagesBuildManifestEntryKind::MetaPackageFile(package_manifest_file) => {
                        deps.insert(package_manifest_file.as_str());
                    }
                }
                deps.insert(package_manifest_file.as_str());
            }
        }

        let dep_paths = deps.iter().map(|x| x.to_string()).collect::<BTreeSet<String>>();
        let depfile_path = cmd.out.join(META_FAR_DEPFILE_NAME);

        write_depfile(depfile_path.as_std_path(), meta_far_path.as_path(), dep_paths.into_iter())?;
    }

    // FIXME(https://fxbug.dev/42052115): Some tools still depend on the legacy `blobs.json` file. We
    // should migrate them over to using `package_manifest.json` so we can stop producing this file.
    if cmd.blobs_json {
        let blobs_json_path = cmd.out.join(BLOBS_JSON_NAME);

        let mut tmp = NamedTempFile::new_in(&cmd.out)
            .with_context(|| format!("creating {blobs_json_path}"))?;

        to_writer_json_pretty(&mut tmp, package_manifest.blobs())
            .with_context(|| format!("creating {blobs_json_path}"))?;

        tmp.persist_if_changed(&blobs_json_path)
            .with_context(|| format!("creating {blobs_json_path}"))?;
    }

    // FIXME(https://fxbug.dev/42052115): Some tools still depend on the legacy `blobs.manifest` file. We
    // should migrate them over to using `package_manifest.json` so we can stop producing this file.
    if cmd.blobs_manifest {
        let blobs_manifest_path = cmd.out.join(BLOBS_MANIFEST_NAME);

        let mut tmp = NamedTempFile::new_in(&cmd.out)
            .with_context(|| format!("creating {blobs_manifest_path}"))?;

        {
            let mut file = BufWriter::new(&mut tmp);

            for entry in package_manifest.blobs() {
                writeln!(file, "{}={}", entry.merkle, entry.source_path)?;
            }
        }

        tmp.persist_if_changed(&blobs_manifest_path)
            .with_context(|| format!("creating {blobs_manifest_path}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::convert_to_depfile_filepath;
    use camino::{Utf8Path, Utf8PathBuf};
    use fuchsia_pkg::{MetaPackage, MetaSubpackages};
    use fuchsia_url::RelativePackageUrl;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use std::convert::TryInto as _;
    use std::fs::{read_dir, read_to_string};
    use version_history::{AbiRevision, ApiLevel, Status, Version, VersionHistory};

    pub const FAKE_VERSION_HISTORY: VersionHistory = VersionHistory::new(&[
        Version {
            api_level: ApiLevel::from_u32(6),
            abi_revision: AbiRevision::from_u64(0x6f3b9f0c4b2a33ff),
            status: Status::Unsupported,
        },
        Version {
            api_level: ApiLevel::from_u32(7),
            abi_revision: AbiRevision::from_u64(0x481ed4bbfa125507),
            status: Status::Supported,
        },
        Version {
            api_level: ApiLevel::from_u32(8),
            // Unlike the other levels, this is the real ABI revision for
            // API level 8, to remain compatible with a previous version of
            // these tests.
            abi_revision: AbiRevision::from_u64(0xa56735a6690e09d8),
            status: Status::Supported,
        },
        Version {
            api_level: ApiLevel::from_u32(9),
            abi_revision: AbiRevision::from_u64(0x2db0661e7832b33d),
            status: Status::Supported,
        },
        Version {
            api_level: ApiLevel::HEAD,
            abi_revision: AbiRevision::from_u64(0x2db0661e7832b33d),
            status: Status::InDevelopment,
        },
        Version {
            api_level: ApiLevel::PLATFORM,
            abi_revision: AbiRevision::from_u64(0x2db0661e7832b33d),
            status: Status::InDevelopment,
        },
    ]);

    fn file_merkle(path: &Utf8Path) -> fuchsia_merkle::Hash {
        let f = File::open(path).unwrap();
        fuchsia_merkle::root_from_reader(f).unwrap()
    }

    fn read_meta_far_contents(path: &Utf8Path) -> BTreeMap<String, String> {
        let mut metafar = File::open(path).unwrap();
        let mut far_reader = fuchsia_archive::Utf8Reader::new(&mut metafar).unwrap();
        let far_paths = far_reader.list().map(|e| e.path().to_string()).collect::<Vec<_>>();

        let mut far_contents = BTreeMap::new();
        for path in far_paths {
            let contents = far_reader.read_file(&path).unwrap();
            let contents = if path == "meta/fuchsia.abi/abi-revision" {
                AbiRevision::from_bytes(contents.try_into().unwrap()).to_string()
            } else {
                String::from_utf8(contents).unwrap()
            };
            far_contents.insert(path, contents);
        }

        far_contents
    }

    #[fuchsia::test]
    async fn test_package_build_manifest_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();

        let cmd = PackageBuildCommand {
            package_build_manifest_path: root.join("invalid path"),
            out: Utf8PathBuf::from("out"),
            api_level: 8.into(),
            repository: None,
            published_name: None,
            depfile: false,
            blobs_json: false,
            blobs_manifest: false,
            subpackages_build_manifest_path: None,
        };

        assert!(cmd_package_build_with_history(cmd, FAKE_VERSION_HISTORY).await.is_err());
    }

    #[fuchsia::test]
    async fn test_package_manifest_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();
        let out = root.join("out");

        let package_build_manifest_path = root.join("package-build.manifest");
        File::create(&out).unwrap();

        let cmd = PackageBuildCommand {
            package_build_manifest_path,
            out,
            api_level: 8.into(),
            repository: None,
            published_name: None,
            depfile: false,
            blobs_json: false,
            blobs_manifest: false,
            subpackages_build_manifest_path: None,
        };

        assert!(cmd_package_build_with_history(cmd, FAKE_VERSION_HISTORY).await.is_err());
    }

    #[fuchsia::test]
    async fn test_generate_empty_package_manifest() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();
        let out = root.join("out");

        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        package_build_manifest
            .write_all(format!("meta/package={meta_package_path}").as_bytes())
            .unwrap();

        cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: 8.into(),
                repository: None,
                published_name: None,
                depfile: false,
                blobs_json: false,
                blobs_manifest: false,
                subpackages_build_manifest_path: None,
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap();

        let meta_far_path = out.join(META_FAR_NAME);
        let package_manifest_path = out.join(PACKAGE_MANIFEST_NAME);

        // Make sure we only generated what we expected.
        let mut paths = read_dir(&out).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        paths.sort();

        assert_eq!(paths, vec![meta_far_path.clone(), package_manifest_path.clone()]);

        assert_eq!(
            serde_json::from_reader::<_, serde_json::Value>(
                File::open(&package_manifest_path).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "version": "1",
                "package": {
                    "name": "my-package",
                    "version": "0",
                },
                "abi_revision": "0xa56735a6690e09d8",
                "blobs": [
                    {
                        "source_path": meta_far_path,
                        "path": "meta/",
                        "merkle": "436eb4b5943bc74f97d95dc81b2de9acddf6691453a536588a86280cac55222e",
                        "size": 12288,
                    }
                ],
            }),
        );

        assert_eq!(
            read_meta_far_contents(&out.join(META_FAR_NAME)),
            BTreeMap::from([
                ("meta/contents".into(), "".into()),
                ("meta/package".into(), r#"{"name":"my-package","version":"0"}"#.into()),
                (
                    "meta/fuchsia.abi/abi-revision".into(),
                    // ABI revision for API level 8.
                    "a56735a6690e09d8".into()
                ),
            ]),
        );
    }

    #[fuchsia::test]
    async fn test_generate_empty_package_manifest_api_level_head() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();
        let out = root.join("out");

        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        package_build_manifest
            .write_all(format!("meta/package={meta_package_path}").as_bytes())
            .unwrap();

        cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: ApiLevel::HEAD,
                repository: None,
                published_name: None,
                depfile: false,
                blobs_json: false,
                blobs_manifest: false,
                subpackages_build_manifest_path: None,
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap();

        let meta_far_path = out.join(META_FAR_NAME);
        let package_manifest_path = out.join(PACKAGE_MANIFEST_NAME);

        // Make sure we only generated what we expected.
        let mut paths = read_dir(&out).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        paths.sort();

        assert_eq!(paths, vec![meta_far_path.clone(), package_manifest_path.clone()]);

        // Since we're generating a file with the latest ABI revision, the meta.far merkle might
        // change when we roll the ABI. So compute the merkle of the file.
        let meta_far_merkle_file = File::open(&meta_far_path).unwrap();
        let meta_far_size = meta_far_merkle_file.metadata().unwrap().len();
        let meta_far_merkle = fuchsia_merkle::root_from_reader(meta_far_merkle_file).unwrap();

        assert_eq!(
            serde_json::from_reader::<_, serde_json::Value>(
                File::open(&package_manifest_path).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "version": "1",
                "package": {
                    "name": "my-package",
                    "version": "0",
                },
                "abi_revision": "0x2db0661e7832b33d",
                "blobs": [
                    {
                        "source_path": meta_far_path,
                        "path": "meta/",
                        "merkle": meta_far_merkle.to_string(),
                        "size": meta_far_size,
                    }
                ],
            }),
        );

        assert_eq!(
            read_meta_far_contents(&out.join(META_FAR_NAME)),
            BTreeMap::from([
                ("meta/contents".into(), "".into()),
                ("meta/package".into(), r#"{"name":"my-package","version":"0"}"#.into()),
                ("meta/fuchsia.abi/abi-revision".into(), "2db0661e7832b33d".to_string()),
            ]),
        );
    }

    #[fuchsia::test]
    async fn test_build_package_with_contents() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();

        let out = root.join("out");

        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        let empty_file_path = root.join("empty-file");
        File::create(&empty_file_path).unwrap();

        package_build_manifest
            .write_all(
                format!("empty-file={empty_file_path}\nmeta/package={meta_package_path}\n",)
                    .as_bytes(),
            )
            .unwrap();

        cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: 8.into(),
                repository: Some("my-repository".into()),
                published_name: None,
                depfile: false,
                blobs_json: false,
                blobs_manifest: false,
                subpackages_build_manifest_path: None,
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap();

        let meta_far_path = out.join(META_FAR_NAME);
        let package_manifest_path = out.join(PACKAGE_MANIFEST_NAME);

        // Make sure we only generated what we expected.
        let mut paths = read_dir(&out).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        paths.sort();

        assert_eq!(paths, vec![meta_far_path.clone(), package_manifest_path.clone()]);

        assert_eq!(
            serde_json::from_reader::<_, serde_json::Value>(
                File::open(&package_manifest_path).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "version": "1",
                "package": {
                    "name": "my-package",
                    "version": "0",
                },
                "abi_revision": "0xa56735a6690e09d8",
                "repository": "my-repository",
                "blobs": [
                    {
                        "source_path": meta_far_path,
                        "path": "meta/",
                        "merkle": "36dde5da0ed4a51433a3b45ed9917c98442613f4b12e0f9661519678482ab3e3",
                        "size": 16384,
                    },
                    {
                        "source_path": empty_file_path,
                        "path": "empty-file",
                        "merkle": "15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b",
                        "size": 0,
                    },
                ],
            }),
        );

        assert_eq!(
            read_meta_far_contents(&meta_far_path),
            BTreeMap::from([
                (
                    "meta/contents".into(),
                    "empty-file=15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b\n"
                        .into(),
                ),
                ("meta/package".into(), r#"{"name":"my-package","version":"0"}"#.into()),
                (
                    "meta/fuchsia.abi/abi-revision".into(),
                    // ABI revision for API level 8.
                    "a56735a6690e09d8".into()
                ),
            ]),
        );
    }

    #[fuchsia::test]
    async fn test_build_package_with_everything() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();

        let out = root.join("out");

        // Write the MetaPackage file.
        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        // Write the subpackages build manifest file, matching the schema from
        // //src/sys/pkg/lib/fuchsia-pkg/src/subpackages_build_manifest.rs
        let subpackages_build_manifest_path = root.join("subpackages");
        let subpackages_build_manifest_file =
            File::create(&subpackages_build_manifest_path).unwrap();
        let subpackage_url = "mock-subpackage".parse::<RelativePackageUrl>().unwrap();
        let subpackage_hash = fuchsia_merkle::Hash::from([0; fuchsia_merkle::HASH_SIZE]);

        let subpackage_package_manifest_file = root.join("subpackage_package_manifest.json");
        let subpackage_package_manifest_file_file =
            File::create(&subpackage_package_manifest_file).unwrap();
        serde_json::to_writer(
            subpackage_package_manifest_file_file,
            &serde_json::json!(
                {
                    "package": {
                        "name": "mock-subpackage",
                        "version": "0"
                    },
                    "abi_revision": "0xa56735a6690e09d8",
                    "blobs": [
                        {
                            "path": "meta/",
                            "merkle": "0000000000000000000000000000000000000000000000000000000000000000",
                            "size": 0,
                            "source_path": "../../blobs/0000000000000000000000000000000000000000000000000000000000000000"
                        }
                    ],
                    "version": "1",
                    "blob_sources_relative": "file",
                    "subpackages": [],
                    "repository": "fuchsia.com"
                }
            ),
        )
        .unwrap();

        let meta_subpackages = MetaSubpackages::from_iter([(subpackage_url, subpackage_hash)]);
        let meta_subpackages_str = serde_json::to_string(&meta_subpackages).unwrap();

        serde_json::to_writer(
            subpackages_build_manifest_file,
            &serde_json::json!([
                {
                    "package_manifest_file": subpackage_package_manifest_file.to_string(),
                }
            ]),
        )
        .unwrap();

        // Write the creation manifest file.
        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        let empty_file_path = root.join("empty-file");
        std::fs::write(&empty_file_path, b"").unwrap();

        // Compute the empty-file's expected hash.
        let empty_file_hash = file_merkle(&empty_file_path);

        package_build_manifest
            .write_all(
                format!("empty-file={empty_file_path}\nmeta/package={meta_package_path}")
                    .as_bytes(),
            )
            .unwrap();

        // Build the package.
        cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: 9.into(),
                repository: None,
                published_name: Some("published-name".into()),
                depfile: true,
                blobs_json: true,
                blobs_manifest: true,
                subpackages_build_manifest_path: Some(subpackages_build_manifest_path.clone()),
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap();

        let meta_far_path = out.join(META_FAR_NAME);

        // Compute the meta.far's hash.
        let meta_far_hash = file_merkle(&meta_far_path);

        // Make sure the meta.far is correct.
        assert_eq!(
            read_meta_far_contents(&meta_far_path),
            BTreeMap::from([
                ("meta/contents".into(), format!("empty-file={empty_file_hash}\n")),
                ("meta/package".into(), r#"{"name":"my-package","version":"0"}"#.into()),
                ("meta/fuchsia.abi/abi-revision".into(), "2db0661e7832b33d".to_string(),),
                ("meta/fuchsia.pkg/subpackages".into(), meta_subpackages_str),
            ]),
        );

        let meta_far_path = out.join(META_FAR_NAME);
        let package_manifest_path = out.join(PACKAGE_MANIFEST_NAME);
        let meta_far_depfile_path = out.join(META_FAR_DEPFILE_NAME);
        let blobs_json_path = out.join(BLOBS_JSON_NAME);
        let blobs_manifest_path = out.join(BLOBS_MANIFEST_NAME);

        // Make sure we only generated what we expected.
        let mut paths = read_dir(&out).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        paths.sort();

        assert_eq!(
            paths,
            vec![
                blobs_json_path.clone(),
                blobs_manifest_path.clone(),
                meta_far_path.clone(),
                meta_far_depfile_path.clone(),
                package_manifest_path,
            ],
        );

        // Make sure the depfile is correct.
        let expected = [
            convert_to_depfile_filepath(subpackages_build_manifest_path.as_str()),
            convert_to_depfile_filepath(empty_file_path.as_str()),
            convert_to_depfile_filepath(meta_package_path.as_str()),
            convert_to_depfile_filepath(subpackage_package_manifest_file.as_str()),
        ];

        assert_eq!(
            read_to_string(meta_far_depfile_path).unwrap(),
            format!(
                "{}: {}",
                convert_to_depfile_filepath(meta_far_path.as_str()),
                expected
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        );

        assert_eq!(
            serde_json::from_reader::<_, serde_json::Value>(File::open(&blobs_json_path).unwrap())
                .unwrap(),
            serde_json::json!([
                    {
                        "source_path": meta_far_path,
                        "path": "meta/",
                        "merkle": meta_far_hash,
                        "size": 20480,
                    },
                    {
                        "source_path": empty_file_path,
                        "path": "empty-file",
                        "merkle": empty_file_hash,
                        "size": 0,
                    },
                ]
            )
        );

        assert_eq!(
            read_to_string(blobs_manifest_path).unwrap(),
            format!(
                "{meta_far_hash}={meta_far_path}\n\
                {empty_file_hash}={empty_file_path}\n"
            ),
        );
    }

    #[fuchsia::test]
    async fn test_build_package_with_abi_revision_file_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();

        let out = root.join("out");

        // Write the MetaPackage file.
        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        // Write the creation manifest file.
        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        // Create a properly-formatted ABI revision file.
        let abi_stamp_path = root.join("abi_stamp");
        std::fs::write(&abi_stamp_path, "2db0661e7832b33d").unwrap();

        package_build_manifest
            .write_all(
                format!(
                    "meta/package={meta_package_path}\n\
                     meta/fuchsia.abi/abi-revision={abi_stamp_path}\n"
                )
                .as_bytes(),
            )
            .unwrap();

        // Building the package will fail, because the ABI revision must be
        // specified on the command line (via the --api-level flag), rather than
        // via the manifest.
        let err = cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: ApiLevel::HEAD,
                repository: None,
                published_name: Some("published-name".into()),
                depfile: true,
                blobs_json: true,
                blobs_manifest: true,
                subpackages_build_manifest_path: None,
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap_err();

        let err_string = format!("{err:?}");
        assert!(err_string.contains("--api-level"), "Wrong error message: {err_string}");
    }

    #[fuchsia::test]
    async fn test_build_package_unknown_api_level() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();
        let out = root.join("out");

        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        package_build_manifest
            .write_all(format!("meta/package={meta_package_path}").as_bytes())
            .unwrap();

        let err = cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: 3221225472.into(), // Arbitrary big number.
                repository: None,
                published_name: None,
                depfile: false,
                blobs_json: false,
                blobs_manifest: false,
                subpackages_build_manifest_path: None,
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap_err();

        // Ensure the error mentions the API level.
        let err_string = format!("{err:?}");
        assert!(err_string.contains("3221225472"), "Wrong error message: {err_string}");
    }

    #[fuchsia::test]
    async fn test_build_package_unsupported_api_level() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tempdir.path()).unwrap();
        let out = root.join("out");

        let meta_package_path = root.join("package");
        let meta_package_file = File::create(&meta_package_path).unwrap();
        let meta_package = MetaPackage::from_name_and_variant_zero("my-package".parse().unwrap());
        meta_package.serialize(meta_package_file).unwrap();

        let package_build_manifest_path = root.join("package-build.manifest");
        let mut package_build_manifest = File::create(&package_build_manifest_path).unwrap();

        package_build_manifest
            .write_all(format!("meta/package={meta_package_path}").as_bytes())
            .unwrap();

        let err = cmd_package_build_with_history(
            PackageBuildCommand {
                package_build_manifest_path,
                out: out.clone(),
                api_level: 6.into(),
                repository: None,
                published_name: None,
                depfile: false,
                blobs_json: false,
                blobs_manifest: false,
                subpackages_build_manifest_path: None,
            },
            FAKE_VERSION_HISTORY,
        )
        .await
        .unwrap_err();

        let err_string = format!("{err:?}");
        assert!(
            err_string.contains("no longer supports API level 6"),
            "Wrong error message: {err_string}"
        );
    }
}
