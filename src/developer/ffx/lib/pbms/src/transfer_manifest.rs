// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Download files referenced in a Transfer Manifest JSON file.
//!
//! This builds upon the lower level /src/lib/transfer_manifest lib.

use crate::pbms::{GS_SCHEME, fetch_from_url};
use crate::{AuthFlowChoice, string_from_url};
use ::gcs::client::{Client, ProgressResult, ProgressState};
use ::gcs::gs_url::split_gs_url;

use futures::{StreamExt as _, TryStreamExt as _};
use std::format;
use std::path::{Component, Path, PathBuf};
use transfer_manifest::{TransferManifest, TransferManifestV1};

/// Download a set of files referenced in the `transfer_manifest_url`.
///
/// Files will be nested under `local_dir` which must exist when this function
/// is called.
pub async fn transfer_download<F, I>(
    transfer_manifest_url: &url::Url,
    local_dir: &Path,
    auth_flow: &AuthFlowChoice,
    progress: &F,
    ui: &I,
    client: &Client,
) -> Result<(), crate::PbmsError>
where
    F: Fn(Vec<ProgressState<'_>>) -> ProgressResult,
    I: structured_ui::Interface,
{
    let start = std::time::Instant::now();
    log::debug!(
        "transfer_download, transfer_manifest_url {:?}, local_dir {:?}",
        transfer_manifest_url,
        local_dir
    );
    assert!(local_dir.is_dir());

    let tm = string_from_url(&transfer_manifest_url, auth_flow, &|f| progress(vec![f]), ui, client)
        .await?;

    let manifest = serde_json::from_str::<TransferManifest>(&tm)?;
    match &manifest {
        TransferManifest::V1(v1_data) => {
            transfer_download_v1(
                transfer_manifest_url,
                v1_data,
                local_dir,
                auth_flow,
                progress,
                ui,
                client,
            )
            .await?
        }
    }
    log::debug!("Total fetch images runtime {} seconds.", start.elapsed().as_secs_f32());
    Ok(())
}

/// Join `relative` onto `base` with light path normalization.
/// Errors out if the final path is not within `base`.
pub(crate) fn safe_join(base: &Path, relative: &Path) -> Result<PathBuf, crate::PbmsError> {
    let mut normalized_relative = PathBuf::new();
    for part in relative.components() {
        match part {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if !normalized_relative.pop() {
                    log::warn!("Failed to normalize path: {}", relative.to_string_lossy());
                    break;
                }
            }
            Component::Normal(part_str) => normalized_relative.push(part_str),
        }
    }

    if normalized_relative.parent().is_some() {
        Ok(base.join(normalized_relative))
    } else {
        return Err(crate::PbmsError::UnsafeConcat(
            relative.to_string_lossy().to_string(),
            base.to_string_lossy().to_string(),
        ));
    }
}

/// Helper for transfer_download specifically for version 1 transfer manifests.
///
/// Files will be nested under `local_dir` which must exist when this function
/// is called.

async fn transfer_download_v1<F, I>(
    transfer_manifest_url: &url::Url,
    transfer_manifest: &TransferManifestV1,
    local_dir: &Path,
    auth_flow: &AuthFlowChoice,
    progress: &F,
    ui: &I,
    client: &Client,
) -> Result<(), crate::PbmsError>
where
    F: Fn(Vec<ProgressState<'_>>) -> ProgressResult,
    I: structured_ui::Interface,
{
    fn malformed_warning<T>(err: T) -> T {
        eprintln!("The specified remote transfer manifest is malformed or broken.");
        eprintln!(
            "This can happen when you're trying to download a broken or unsupported build/release."
        );
        eprintln!("Please try a different product bundle transfer url.");
        err
    }

    let base_url = match transfer_manifest_url.scheme() {
        GS_SCHEME => format!(
            "gs://{}",
            split_gs_url(&transfer_manifest_url.as_str())
                .map_err(|_| crate::PbmsError::InvalidGsUrl(transfer_manifest_url.to_string()))?
                .0
        ),
        _ => transfer_manifest_url[..url::Position::BeforePath].to_string(),
    };
    let mut tasks = Vec::new();
    let transfer_entry_count = transfer_manifest.entries.len() as u64;
    for (i, transfer_entry) in transfer_manifest.entries.iter().enumerate() {
        let te_remote_dir = match url::Url::parse(transfer_entry.remote.as_str()) {
            Ok(url) => url.to_string(),
            // Avoid using base_url.join().
            Err(_) => format!("{}/{}", base_url, transfer_entry.remote.as_str()),
        };

        let te_local_dir =
            safe_join(&local_dir, transfer_entry.local.as_std_path()).map_err(malformed_warning)?;
        let artifact_entry_count = transfer_entry.entries.len() as u64;
        for (k, artifact_entry) in transfer_entry.entries.iter().enumerate() {
            // Avoid using te_remote_dir.join().
            let remote_file = url::Url::parse(&format!(
                "{}/{}",
                te_remote_dir.trim_end_matches('/'),
                artifact_entry.name.as_str()
            ))?;

            let local_file = safe_join(&te_local_dir, artifact_entry.name.as_std_path())
                .map_err(malformed_warning)?;
            let local_parent = local_file
                .parent()
                .ok_or_else(|| crate::PbmsError::NoParent(local_file.to_path_buf()))?
                .to_path_buf();
            async_fs::create_dir_all(&local_parent).await?;

            log::debug!("Transfer {:?} to {:?}", remote_file, local_parent);
            tasks.push(async move {
                fetch_from_url(
                    &remote_file,
                    local_parent,
                    auth_flow,
                    &|_, f| {
                        // The directory progress is replaced because a transfer
                        // manifest refers to specific files to copy (not
                        // directories at time), so the directory progress is
                        // always "1 of 1 files", which is not helpful.
                        let section = ProgressState {
                            name: &transfer_entry.remote.as_str(),
                            at: i as u64 + 1,
                            of: transfer_entry_count,
                            units: "sections",
                        };
                        let directory = ProgressState {
                            name: &remote_file.as_str(),
                            at: k as u64,
                            of: artifact_entry_count,
                            units: "files",
                        };
                        progress(vec![section, directory, f])
                    },
                    ui,
                    &client.clone(),
                )
                .await
            })
        }
    }
    let mut stream = futures::stream::iter(tasks.into_iter()).buffer_unordered(
        std::thread::available_parallelism().map_err(|e| crate::PbmsError::Io(e))?.get(),
    );

    while let Some(()) = stream.try_next().await? {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::collections::HashMap;
    use transfer_manifest::{ArtifactEntry, ArtifactType, TransferEntry};

    /// Helper function to filter the artifacts need to be downloaded.
    fn transfer_filter(
        manifest: &TransferManifestV1,
        rule: &HashMap<(ArtifactType, Utf8PathBuf), Vec<Utf8PathBuf>>,
    ) -> TransferManifestV1 {
        let mut transfer_entries = Vec::new();
        for manifest_transfer_entry in &manifest.entries {
            let prefixes = rule.get(&(
                manifest_transfer_entry.artifact_type.clone(),
                manifest_transfer_entry.local.clone(),
            ));
            if prefixes.is_none() {
                continue;
            }
            let mut filtered_artifact_entries = Vec::new();
            for manifest_artifact_entry in &manifest_transfer_entry.entries {
                if prefixes
                    .unwrap()
                    .iter()
                    .any(|prefix| manifest_artifact_entry.name.starts_with(prefix))
                {
                    filtered_artifact_entries.push(manifest_artifact_entry.clone());
                }
            }
            if filtered_artifact_entries.len() != 0 {
                transfer_entries.push(TransferEntry {
                    entries: filtered_artifact_entries,
                    ..manifest_transfer_entry.clone()
                });
            }
        }
        TransferManifestV1 { entries: transfer_entries }
    }

    #[test]
    fn test_safe_join() -> std::result::Result<(), anyhow::Error> {
        macro_rules! assert_joined {
            ($base:literal, $relative:literal, $result:literal) => {
                assert_eq!(safe_join(Path::new($base), Path::new($relative))?, Path::new($result));
            };
        }

        macro_rules! assert_cannot_join {
            ($base:literal, $relative:literal) => {
                assert_eq!(
                    safe_join(Path::new($base), Path::new($relative)).unwrap_err().to_string(),
                    concat!("Cannot safely concat ", $relative, " onto ", $base),
                );
            };
        }

        // absolute / relative
        assert_joined!("/absolute", "relative", "/absolute/relative");

        // relative / relative
        assert_joined!("./relative", "./subdir", "./relative/subdir");
        assert_joined!("relative", "subdir", "relative/subdir");

        // Join with subdirs.
        assert_joined!("/absolute/subdir", "relative/subdir", "/absolute/subdir/relative/subdir");

        // Second path is normalized.
        assert_joined!(
            "/absolute/subdir/..",
            "relative/subdir/../subdir",
            "/absolute/subdir/../relative/subdir"
        );

        // Second path is treated as relative
        assert_joined!("/absolute", "/not_absolute", "/absolute/not_absolute");

        // Subpath must be non-empty.
        assert_cannot_join!("./relative", "");
        assert_cannot_join!("./relative", ".");
        assert_cannot_join!("./relative", "./subdir/..");

        // Subpath cannot escape the base directory.
        assert_cannot_join!("/absolute/path", "..");
        assert_cannot_join!("/absolute/path", "/../path");
        assert_cannot_join!("/absolute/path", "a_dir/../..");

        #[cfg(target_os = "windows")]
        {
            assert_joined!(r"C:\absolute", "C:evil.txt", r"C:\absolute\evil.txt");
            assert_joined!(r"C:\absolute", r"C:\evil.txt", r"C:\absolute\evil.txt");
            assert_cannot_join!(r"C:\absolute", "C:");
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert_joined!("/absolute", "C:evil.txt", "/absolute/C:evil.txt");
        }

        Ok(())
    }

    #[fuchsia::test]
    fn test_filter_transfer_manifest_for_repository_artifacts() {
        let original_manifest = TransferManifestV1 {
            entries: vec![
                TransferEntry {
                    artifact_type: ArtifactType::Blobs,
                    local: Utf8PathBuf::from("product_bundle/blobs"),
                    remote: "gs://bucket/a".into(),
                    entries: vec![
                        ArtifactEntry {
                            name: Utf8PathBuf::from(
                                "002e9903f92959080ee3abf0a1fce9db76fe0c2ea9a89c40813658cf82faa2bc",
                            ),
                        },
                        ArtifactEntry {
                            name: Utf8PathBuf::from(
                                "00f8ba8acab8d585c663f4271563e0e2f7e7ee566a68296620be2461eef497b5",
                            ),
                        },
                    ],
                },
                TransferEntry {
                    artifact_type: ArtifactType::Files,
                    local: Utf8PathBuf::from("product_bundle"),
                    remote: "gs://bucket/b".into(),
                    entries: vec![
                        ArtifactEntry {
                            name: Utf8PathBuf::from("partitions/gpt.fuchsia.3728.bin"),
                        },
                        ArtifactEntry { name: Utf8PathBuf::from("repository/1.root.json") },
                        ArtifactEntry { name: Utf8PathBuf::from("product_bundle.json") },
                        ArtifactEntry { name: Utf8PathBuf::from("system_a/fuchsia.vbmeta") },
                        ArtifactEntry { name: Utf8PathBuf::from("system_a/fuchsia.zbi") },
                    ],
                },
            ],
        };

        let mut rule = HashMap::new();
        rule.insert(
            (ArtifactType::Blobs, Utf8PathBuf::from("product_bundle/blobs")),
            vec![Utf8PathBuf::from("")],
        );
        rule.insert(
            (ArtifactType::Files, Utf8PathBuf::from("product_bundle")),
            vec![
                Utf8PathBuf::from("partitions"),
                Utf8PathBuf::from("repository"),
                Utf8PathBuf::from("product_bundle.json"),
            ],
        );

        let expected = TransferManifestV1 {
            entries: vec![
                TransferEntry {
                    artifact_type: ArtifactType::Blobs,
                    local: Utf8PathBuf::from("product_bundle/blobs"),
                    remote: "gs://bucket/a".into(),
                    entries: vec![
                        ArtifactEntry {
                            name: Utf8PathBuf::from(
                                "002e9903f92959080ee3abf0a1fce9db76fe0c2ea9a89c40813658cf82faa2bc",
                            ),
                        },
                        ArtifactEntry {
                            name: Utf8PathBuf::from(
                                "00f8ba8acab8d585c663f4271563e0e2f7e7ee566a68296620be2461eef497b5",
                            ),
                        },
                    ],
                },
                TransferEntry {
                    artifact_type: ArtifactType::Files,
                    local: Utf8PathBuf::from("product_bundle"),
                    remote: "gs://bucket/b".into(),
                    entries: vec![
                        ArtifactEntry {
                            name: Utf8PathBuf::from("partitions/gpt.fuchsia.3728.bin"),
                        },
                        ArtifactEntry { name: Utf8PathBuf::from("repository/1.root.json") },
                        ArtifactEntry { name: Utf8PathBuf::from("product_bundle.json") },
                    ],
                },
            ],
        };
        let actual = transfer_filter(&original_manifest, &rule);
        assert_eq!(expected, actual)
    }

    #[fuchsia::test]
    fn test_filter_transfer_manifest_for_flash_artifacts() {
        let original_manifest = TransferManifestV1 {
            entries: vec![
                TransferEntry {
                    artifact_type: ArtifactType::Blobs,
                    local: Utf8PathBuf::from("product_bundle/blobs"),
                    remote: "gs://bucket/a".into(),
                    entries: vec![
                        ArtifactEntry {
                            name: Utf8PathBuf::from(
                                "002e9903f92959080ee3abf0a1fce9db76fe0c2ea9a89c40813658cf82faa2bc",
                            ),
                        },
                        ArtifactEntry {
                            name: Utf8PathBuf::from(
                                "00f8ba8acab8d585c663f4271563e0e2f7e7ee566a68296620be2461eef497b5",
                            ),
                        },
                    ],
                },
                TransferEntry {
                    artifact_type: ArtifactType::Files,
                    local: Utf8PathBuf::from("product_bundle"),
                    remote: "gs://bucket/b".into(),
                    entries: vec![
                        ArtifactEntry {
                            name: Utf8PathBuf::from("partitions/gpt.fuchsia.3728.bin"),
                        },
                        ArtifactEntry { name: Utf8PathBuf::from("repository/1.root.json") },
                        ArtifactEntry { name: Utf8PathBuf::from("product_bundle.json") },
                        ArtifactEntry { name: Utf8PathBuf::from("system_a/fuchsia.vbmeta") },
                        ArtifactEntry { name: Utf8PathBuf::from("system_a/fuchsia.zbi") },
                    ],
                },
            ],
        };

        let mut rule = HashMap::new();
        rule.insert(
            (ArtifactType::Files, Utf8PathBuf::from("product_bundle")),
            vec![
                Utf8PathBuf::from("partitions"),
                Utf8PathBuf::from("system_a"),
                Utf8PathBuf::from("product_bundle.json"),
            ],
        );

        let expected = TransferManifestV1 {
            entries: vec![TransferEntry {
                artifact_type: ArtifactType::Files,
                local: Utf8PathBuf::from("product_bundle"),
                remote: "gs://bucket/b".into(),
                entries: vec![
                    ArtifactEntry { name: Utf8PathBuf::from("partitions/gpt.fuchsia.3728.bin") },
                    ArtifactEntry { name: Utf8PathBuf::from("product_bundle.json") },
                    ArtifactEntry { name: Utf8PathBuf::from("system_a/fuchsia.vbmeta") },
                    ArtifactEntry { name: Utf8PathBuf::from("system_a/fuchsia.zbi") },
                ],
            }],
        };
        let actual = transfer_filter(&original_manifest, &rule);
        assert_eq!(expected, actual)
    }
}
