// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{BoardArgs, HybridBoardArgs};

use anyhow::{Context, Result};
use assembly_config_schema::{BibReference, BoardConfig, BoardInputBundleSet};
use assembly_container::{AssemblyContainer, DirectoryPathBuf};
use assembly_partitions_config::PartitionsConfig;
use assembly_release_info::{BoardReleaseInfo, ReleaseInfo};
use assembly_util::{get_release_repository, get_release_version, validate_release_info_string};
use std::collections::BTreeMap;

pub fn new(args: &BoardArgs) -> Result<()> {
    let mut config = BoardConfig::from_config_path(&args.config)?;
    let release_info_name = config.release_info.info.name.clone();

    if let Some(partitions_config) = &args.partitions_config {
        config.partitions_config = Some(DirectoryPathBuf::new(partitions_config.clone()));

        // We must assert that the board name matches the hardware_revision in
        // the partitions config, otherwise OTAs may not work.
        let partitions = PartitionsConfig::from_dir(partitions_config)
            .context("Validating partitions config")?;
        if partitions.hardware_revision != "" {
            // Explicitly assign partitions.hw_revision to the board.name to ensure they match.
            config.name = partitions.hardware_revision;
        }
    }

    for (i, board_input_bundle) in args.board_input_bundles.iter().enumerate() {
        let key = format!("tmp{}", i);
        let directory = DirectoryPathBuf::new(board_input_bundle.clone());
        config.input_bundles.insert(key, directory);
    }

    // Build systems do not know the name of the BIBs, so they serialize index
    // numbers in place of BIB names by default. We add the BIB names in now,
    // so all the rest of the rules can assume the config has proper BIB names.
    let mut config = config.add_bib_names()?;

    // Map of BIB repository to the BIB set.
    let bib_sets: BTreeMap<String, BoardInputBundleSet> = args
        .board_input_bundle_sets
        .iter()
        .map(|path| {
            let bib_set = BoardInputBundleSet::from_dir(&path)?;
            let set_name = bib_set.name.clone();
            Ok((set_name, bib_set))
        })
        .collect::<Result<BTreeMap<String, BoardInputBundleSet>>>()?;

    let mut bib_sets_info: Vec<ReleaseInfo> = vec![];
    // Add all the BIBs from the BIB sets.
    for (set_name, set) in bib_sets {
        bib_sets_info.push(set.release_info.clone());
        for (bib_name, bib_entry) in set.board_input_bundles {
            let bib_ref = BibReference::FromBibSet { set: set_name.clone(), name: bib_name };
            config.input_bundles.insert(bib_ref.to_string(), bib_entry.path);
        }
    }

    let repository = get_release_repository(&args.repo, &args.repo_file)?;
    let version = get_release_version(&args.version, &args.version_file)?;

    config.release_info = BoardReleaseInfo {
        info: ReleaseInfo {
            name: validate_release_info_string(release_info_name)?,
            repository: validate_release_info_string(repository)?,
            version: validate_release_info_string(version)?,
        },
        bib_sets: bib_sets_info,
    };

    config.write_to_dir(&args.output, args.depfile.as_ref())?;
    Ok(())
}

pub fn hybrid(args: &HybridBoardArgs) -> Result<()> {
    let mut config = BoardConfig::from_dir(&args.config)?;

    // First, replace the bibs found in `replace_bibs_from_board`.
    if let Some(replace_bibs_from_board) = &args.replace_bibs_from_board {
        let replace_config = BoardConfig::from_dir(replace_bibs_from_board)?;
        for (name, replacement) in replace_config.input_bundles.into_iter() {
            config.input_bundles.entry(name).and_modify(|bib| *bib = replacement);
        }
    }

    // Second, replace the bibs found in `replace_bib_sets`.
    let replace_bib_sets: Vec<BoardInputBundleSet> = args
        .replace_bib_sets
        .iter()
        .map(|path| BoardInputBundleSet::from_dir(&path))
        .collect::<Result<Vec<BoardInputBundleSet>>>()?;

    config.merge_board_input_bundle_sets(replace_bib_sets);

    // Replace the partitions config.
    if let Some(partitions_config) = &args.replace_partitions_config {
        config.partitions_config = Some(DirectoryPathBuf::new(partitions_config.clone()));
    }

    // Replace the devicetree if specified
    if let Some(devicetree) = &args.replace_devicetree {
        config.devicetree = Some(devicetree.clone());
    }

    config.write_to_dir(&args.output, args.depfile.as_ref())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_config_schema::{BoardInputBundle, BoardInputBundleEntry};
    use assembly_partitions_config::{Partition, Slot};
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;
    use std::fs::File;
    use std::io::Write;
    use tempfile::{NamedTempFile, tempdir};

    #[test]
    fn test_new_board() {
        let config_file = NamedTempFile::new().unwrap();
        let config_path = Utf8PathBuf::from_path_buf(config_file.path().to_path_buf()).unwrap();
        let config_value = serde_json::json!({
            "arch": "x64",
            "name": "my_board",
            "release_info": {
                "info": {
                    "name": "my_board",
                    "repository": "my_repository",
                    "version": "my_version",
                },
                "bib_sets": [],
            }
        });
        serde_json::to_writer(&config_file, &config_value).unwrap();

        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        // Create a BIB.
        let bib_path = tmp_path.join("my_bib");
        let bib = BoardInputBundle {
            name: "my_bib".to_string(),
            kernel_boot_args: ["arg".to_string()].into(),
            ..Default::default()
        };
        bib.write_to_dir(&bib_path, None::<Utf8PathBuf>).unwrap();

        // Add the BIB to a set.
        let bib_set_path = tmp_path.join("my_bib_set");
        let bib_set = BoardInputBundleSet {
            name: "my_bib_set".to_string(),
            board_input_bundles: [(
                "my_bib".to_string(),
                BoardInputBundleEntry { path: DirectoryPathBuf::new(bib_path) },
            )]
            .into(),
            release_info: ReleaseInfo::new_for_testing(),
        };
        bib_set.write_to_dir(&bib_set_path, None::<Utf8PathBuf>).unwrap();

        // Create a board.
        let board_path = tmp_path.join("my_board");
        let args = BoardArgs {
            config: config_path,
            board_input_bundles: vec![],
            board_input_bundle_sets: vec![bib_set_path],
            output: board_path.clone(),
            depfile: None,
            ..Default::default()
        };
        new(&args).unwrap();

        // Ensure the BIB in the board contains the correct kernel_boot_args.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = vec!["my_bib_set::my_bib".to_string()];
        itertools::assert_equal(expected.iter(), board.input_bundles.keys());
        let bib_path = board.input_bundles.get("my_bib_set::my_bib").unwrap();
        let bib = BoardInputBundle::from_dir(bib_path).unwrap();
        let expected = BTreeSet::<String>::from(["arg".to_string()]);
        assert_eq!(expected, bib.kernel_boot_args);
    }

    fn new_board_with_version_repo_fields(
        tmp_path: Utf8PathBuf,
        version: Option<String>,
        version_file: Option<Utf8PathBuf>,
        repo: Option<String>,
        repo_file: Option<Utf8PathBuf>,
    ) -> (Result<(), anyhow::Error>, Utf8PathBuf, Utf8PathBuf) {
        let config_file = NamedTempFile::new().unwrap();
        let config_path = Utf8PathBuf::from_path_buf(config_file.path().to_path_buf()).unwrap();
        let config_value = serde_json::json!({
            "arch": "x64",
            "name": "my_board",
            "release_info": {
                "info": {
                    "name": "my_board",
                    "repository": "my_repository",
                    "version": "my_version",
                },
                "bib_sets": [],
            }
        });
        serde_json::to_writer(&config_file, &config_value).unwrap();

        // Create a BIB.
        let bib_path = tmp_path.join("my_bib");
        let bib = BoardInputBundle {
            name: "my_bib".to_string(),
            kernel_boot_args: ["arg".to_string()].into(),
            ..Default::default()
        };
        bib.write_to_dir(&bib_path, None::<Utf8PathBuf>).unwrap();

        // Add the BIB to a set.
        let bib_set_path = tmp_path.join("my_bib_set");
        let bib_set = BoardInputBundleSet {
            name: "my_bib_set".to_string(),
            board_input_bundles: [(
                "my_bib".to_string(),
                BoardInputBundleEntry { path: DirectoryPathBuf::new(bib_path) },
            )]
            .into(),
            release_info: ReleaseInfo::new_for_testing(),
        };
        bib_set.write_to_dir(&bib_set_path, None::<Utf8PathBuf>).unwrap();

        // Create a board.
        let board_path = tmp_path.join("my_board");
        let args = BoardArgs {
            config: config_path,
            partitions_config: None,
            board_input_bundles: vec![],
            board_input_bundle_sets: vec![bib_set_path.clone()],
            output: board_path.clone(),
            version,
            version_file,
            repo,
            repo_file,
            depfile: None,
        };
        (new(&args), board_path, bib_set_path)
    }

    #[test]
    fn test_new_board_unversioned() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let (_, board_path, _) =
            new_board_with_version_repo_fields(tmp_path, None, None, None, None);

        // Ensure the Board config has the correct version string.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = "unversioned".to_string();
        assert_eq!(expected, board.release_info.info.version);
    }

    #[test]
    fn test_new_board_version_string() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let (_, board_path, _) = new_board_with_version_repo_fields(
            tmp_path,
            Some("fake_version".to_string()),
            None,
            None,
            None,
        );

        // Ensure the Board config has the correct version string.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = "fake_version".to_string();
        assert_eq!(expected, board.release_info.info.version);
    }

    #[test]
    fn test_new_board_version_file() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        let version_file_path = tmp_path.join("version.txt");
        let version_file = File::create(&version_file_path);
        version_file.unwrap().write_all("fake_version".as_bytes()).unwrap();

        let (_, board_path, _) =
            new_board_with_version_repo_fields(tmp_path, None, Some(version_file_path), None, None);

        // Ensure the Board config has the correct version string.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = "fake_version".to_string();
        assert_eq!(expected, board.release_info.info.version);
    }

    #[test]
    fn test_new_board_unknown_repository() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let (_, board_path, _) =
            new_board_with_version_repo_fields(tmp_path, None, None, None, None);

        // Ensure the Board config has the correct repository string.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = "unknown".to_string();
        assert_eq!(expected, board.release_info.info.repository);
    }

    #[test]
    fn test_new_board_repository_string() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let (_, board_path, _) = new_board_with_version_repo_fields(
            tmp_path,
            None,
            None,
            Some("fake_repository".to_string()),
            None,
        );

        // Ensure the Board config has the correct repository string.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = "fake_repository".to_string();
        assert_eq!(expected, board.release_info.info.repository);
    }

    #[test]
    fn test_new_board_repository_file() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        let repo_file_path = tmp_path.join("repo.txt");
        let repo_file = File::create(&repo_file_path);
        repo_file.unwrap().write_all("fake_repository".as_bytes()).unwrap();

        let (_, board_path, _) =
            new_board_with_version_repo_fields(tmp_path, None, None, None, Some(repo_file_path));

        // Ensure the Board config has the correct repository string.
        let board = BoardConfig::from_dir(board_path).unwrap();
        let expected = "fake_repository".to_string();
        assert_eq!(expected, board.release_info.info.repository);
    }

    #[test]
    fn test_hybrid_board() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        // Create a BIB.
        let bib_path = tmp_path.join("my_bib");
        let bib = BoardInputBundle {
            name: "my_bib".to_string(),
            kernel_boot_args: ["before".to_string()].into(),
            ..Default::default()
        };
        bib.write_to_dir(&bib_path, None::<Utf8PathBuf>).unwrap();

        // Create a board with the BIB already added.
        let board_path = tmp_path.join("my_board");
        let board = BoardConfig {
            name: "my_board".to_string(),
            release_info: BoardReleaseInfo::new_for_testing(),
            hardware_info: Default::default(),
            provided_features: Default::default(),
            zbi_extra_items: Default::default(),
            devicetree: Default::default(),
            devicetree_overlay: Default::default(),
            filesystems: Default::default(),
            input_bundles: [("my_bib_set::my_bib".to_string(), DirectoryPathBuf::new(bib_path))]
                .into(),
            configuration: Default::default(),
            kernel: Default::default(),
            platform: Default::default(),
            global_platform_tee_trusted_app_guids: Default::default(),
            ..Default::default()
        };
        board.write_to_dir(&board_path, None::<Utf8PathBuf>).unwrap();

        // Create a new BIB with the same name, but different kernel_boot_args.
        let new_bib_path = tmp_path.join("new_my_bib");
        let bib = BoardInputBundle {
            name: "my_bib".to_string(),
            kernel_boot_args: ["after".to_string()].into(),
            ..Default::default()
        };
        bib.write_to_dir(&new_bib_path, None::<Utf8PathBuf>).unwrap();

        // Add the BIB to a set.
        let bib_set_path = tmp_path.join("my_bib_set");
        let bib_set = BoardInputBundleSet {
            name: "my_bib_set".to_string(),
            board_input_bundles: [(
                "my_bib".to_string(),
                BoardInputBundleEntry { path: DirectoryPathBuf::new(new_bib_path) },
            )]
            .into(),
            release_info: ReleaseInfo::new_for_testing(),
        };
        bib_set.write_to_dir(&bib_set_path, None::<Utf8PathBuf>).unwrap();

        // Write a new partitions config.
        let partitions =
            vec![Partition::ZBI { name: "my_zbi_part".into(), slot: Slot::A, size: None }];
        let partitions_path = tmp_path.join("my_partitions");
        let partitions_config =
            PartitionsConfig { partitions: partitions.clone(), ..Default::default() };
        partitions_config.write_to_dir(&partitions_path, None::<Utf8PathBuf>).unwrap();

        // Write a new devicetree.
        let devicetree_path = tmp_path.join("my_devicetree.dtb");
        let mut devicetree_file = File::create(&devicetree_path).unwrap();
        devicetree_file.write_all(b"foobar").unwrap();

        // Create a hybrid board and replace the BIB using the set.
        let hybrid_board_path = tmp_path.join("my_hybrid_board");
        let args = HybridBoardArgs {
            config: board_path,
            output: hybrid_board_path.clone(),
            replace_bibs_from_board: None,
            replace_bib_sets: vec![bib_set_path],
            replace_partitions_config: Some(partitions_path),
            replace_devicetree: Some(devicetree_path.clone()),
            depfile: None,
        };
        hybrid(&args).unwrap();

        // Ensure the BIB in the board contains the correct kernel_boot_args.
        let board = BoardConfig::from_dir(hybrid_board_path.clone()).unwrap();
        let expected = vec!["my_bib_set::my_bib".to_string()];
        itertools::assert_equal(expected.iter(), board.input_bundles.keys());
        let bib_path = board.input_bundles.get("my_bib_set::my_bib").unwrap();
        let bib = BoardInputBundle::from_dir(bib_path).unwrap();
        let expected = BTreeSet::<String>::from(["after".to_string()]);
        assert_eq!(expected, bib.kernel_boot_args);

        // Ensure the board contains the correct partitions config.
        let new_partitions_path = board.partitions_config.unwrap().as_utf8_path_buf().clone();
        let new_partitions = PartitionsConfig::from_dir(new_partitions_path).unwrap();
        assert_eq!(partitions, new_partitions.partitions);

        // Ensure the board contains the correct devicetree.
        assert_eq!(
            hybrid_board_path.join("devicetree").join("my_devicetree.dtb"),
            board.devicetree.unwrap()
        );
    }
}
