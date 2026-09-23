// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::RepoCreateCommand;
use anyhow::Result;
use fuchsia_repo::repo_builder::RepoBuilder;
use fuchsia_repo::repo_keys::RepoKeys;
use fuchsia_repo::repository::PmRepository;

pub async fn cmd_repo_create(cmd: RepoCreateCommand) -> Result<()> {
    let repo_keys = if let Some(keys_dir) = &cmd.keys {
        RepoKeys::from_dir(keys_dir)?
    } else {
        // If no keys specified, generate keys at {repo_path}/keys.
        let keys_dir = cmd.repo_path.join("keys");
        std::fs::create_dir_all(keys_dir.as_std_path())?;
        RepoKeys::generate(keys_dir.as_std_path())?
    };
    let repo = PmRepository::new(cmd.repo_path);

    RepoBuilder::create(repo, &repo_keys).time_versioning(cmd.time_versioning).commit().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::RepoPublishCommand;
    use crate::repo_publish::cmd_repo_publish;
    use assert_matches::assert_matches;
    use camino::Utf8Path;
    use chrono::Utc;
    use fuchsia_repo::repo_client::RepoClient;
    use fuchsia_repo::repository::CopyMode;
    use pretty_assertions::assert_eq;
    use tuf::metadata::Metadata as _;

    fn default_command_for_test() -> RepoPublishCommand {
        RepoPublishCommand {
            watch: false,
            signing_keys: None,
            trusted_keys: None,
            trusted_root: None,
            package_archives: vec![],
            package_manifests: vec![],
            package_list_manifests: vec![],
            product_bundle: vec![],
            metadata_current_time: Utc::now(),
            time_versioning: false,
            refresh_root: false,
            clean: false,
            depfile: None,
            copy_mode: CopyMode::Copy,
            delivery_blob_type: 1,
            ignore_missing_packages: false,
            blob_manifest: None,
            blob_repo_dir: None,
            repo_path: "".into(),
        }
    }

    #[fuchsia::test]
    async fn test_repository_create_repo_and_generate_keys() {
        let subdirs = vec![
            // Run a test in the tempdir as provided ...
            "",
            // ... and in a to-be-created subdir
            "nonexistent",
        ];
        for subdir in subdirs {
            let tempdir = tempfile::tempdir().unwrap();
            let fqdir = tempdir.path().join(subdir);
            let root = Utf8Path::from_path(fqdir.as_path()).unwrap();

            // Creates repo, including generating keys.
            let repo_create_cmd = RepoCreateCommand {
                repo_path: root.to_path_buf(),
                keys: None,
                time_versioning: false,
            };
            assert_matches!(cmd_repo_create(repo_create_cmd).await, Ok(()));

            let repo_keys_path = root.join("keys");
            let repo_path = root.join("repo");

            let cmd = RepoPublishCommand {
                trusted_keys: Some(repo_keys_path),
                repo_path: repo_path.to_path_buf(),
                ..default_command_for_test()
            };

            assert_matches!(cmd_repo_publish(cmd).await, Ok(()));

            let repo = PmRepository::new(repo_path);
            let mut repo_client = RepoClient::from_trusted_remote(repo).await.unwrap();

            assert_matches!(repo_client.update().await, Ok(true));
            assert_eq!(repo_client.database().trusted_root().version(), 1);
            assert_eq!(repo_client.database().trusted_targets().map(|m| m.version()), Some(1));
            assert_eq!(repo_client.database().trusted_snapshot().map(|m| m.version()), Some(1));
            assert_eq!(repo_client.database().trusted_timestamp().map(|m| m.version()), Some(1));
        }
    }
}
