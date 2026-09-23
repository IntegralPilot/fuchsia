// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BoardInputBundleSetArgs;
use anyhow::Result;
use assembly_config_schema::{BoardInputBundle, BoardInputBundleEntry, BoardInputBundleSet};
use assembly_container::{AssemblyContainer, DirectoryPathBuf};
use assembly_release_info::ReleaseInfo;
use assembly_util::{get_release_repository, get_release_version, validate_release_info_string};
use std::collections::BTreeMap;

pub fn new(args: &BoardInputBundleSetArgs) -> Result<()> {
    let name = args.name.clone();
    let board_input_bundles: BTreeMap<String, BoardInputBundleEntry> = args
        .board_input_bundles
        .iter()
        .map(|path| {
            let bib = BoardInputBundle::from_dir(&path)?;
            let directory = DirectoryPathBuf::new(path.clone());
            let entry = BoardInputBundleEntry { path: directory };
            Ok((bib.name, entry))
        })
        .collect::<Result<BTreeMap<String, BoardInputBundleEntry>>>()?;

    let repository = get_release_repository(&args.repo, &args.repo_file)?;
    let version = get_release_version(&args.version, &args.version_file)?;

    let set = BoardInputBundleSet {
        name: name.clone(),
        board_input_bundles,
        release_info: ReleaseInfo {
            name: validate_release_info_string(name)?,
            repository: validate_release_info_string(repository)?,
            version: validate_release_info_string(version)?,
        },
    };
    set.write_to_dir(&args.output, args.depfile.as_ref())?;
    Ok(())
}
