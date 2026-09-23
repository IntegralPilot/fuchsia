// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::dimension::Dimension;
use crate::search_space::SearchSpace;
use anyhow::{Context, Result, ensure};
use assembly_artifact_cache::{MOSClient, MOSIdentifier, Slot};
use async_trait::async_trait;
use camino::Utf8Path;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// A trait to abstract the MOS client, allowing for mock implementations in tests.
#[async_trait(?Send)]
pub trait MOSClientTrait {
    /// Retrieve release information for a product bundle.
    async fn get_pb_release_info(
        &mut self,
        name: String,
        version: String,
    ) -> Result<Vec<MOSIdentifier>>;

    /// Interpolate between two artifact versions to get a list of all intermediate versions.
    async fn interpolate(
        &self,
        start: &MOSIdentifier,
        end: &MOSIdentifier,
    ) -> Result<Vec<MOSIdentifier>>;
}

#[async_trait(?Send)]
impl MOSClientTrait for MOSClient {
    async fn get_pb_release_info(
        &mut self,
        name: String,
        version: String,
    ) -> Result<Vec<MOSIdentifier>> {
        MOSClient::get_pb_release_info(self, name, version).await
    }

    async fn interpolate(
        &self,
        start: &MOSIdentifier,
        end: &MOSIdentifier,
    ) -> Result<Vec<MOSIdentifier>> {
        MOSClient::interpolate(self, start, end).await
    }
}

/// Retrieves the full search space for bisection by talking to MOS.
pub async fn get_search_space<T: MOSClientTrait, PrintFn>(
    client: &mut T,
    pb_name: &str,
    from_success: &str,
    to_failure: &str,
    fuchsia_dir: Option<&Utf8Path>,
    slot: Slot,
    mut print_fn: PrintFn,
) -> Result<SearchSpace>
where
    PrintFn: FnMut(&str),
{
    print_fn("Preparing bisection plan...");

    let mut starting_artifacts =
        get_pb_release_info(client, pb_name, from_success, &mut print_fn).await?;
    starting_artifacts.retain(|a| {
        a.slot == slot
            && matches!(
                a.artifact_type,
                // TODO(https://fxbug.dev/477619542): Bisect all artifact types.
                assembly_artifact_cache::ArtifactType::Platform
                    | assembly_artifact_cache::ArtifactType::Product
                    | assembly_artifact_cache::ArtifactType::Board
            )
    });

    let mut ending_artifacts =
        get_pb_release_info(client, pb_name, to_failure, &mut print_fn).await?;
    ending_artifacts.retain(|a| {
        a.slot == slot
            && matches!(
                a.artifact_type,
                // TODO(https://fxbug.dev/477619542): Bisect all artifact types.
                assembly_artifact_cache::ArtifactType::Platform
                    | assembly_artifact_cache::ArtifactType::Product
                    | assembly_artifact_cache::ArtifactType::Board
            )
    });

    match interpolate(client, &starting_artifacts, &ending_artifacts, &mut print_fn).await {
        Ok(space) => Ok(space),
        Err(e) => {
            if format!("{:?}", e).contains("*** No Common Ancestor ***") {
                print_fn("\n=======================================================");
                print_fn("Running automated broken link-finding script...");
                print_fn("=======================================================\n");

                // TODO(https://fxbug.dev/495619145): Investigate ways to
                // gracefully handle failure conditions without having to
                // hardcode script paths into the tool like this.
                let script_path = fuchsia_dir.map(|d| {
                    d.join(
                        "vendor/google/scripts/ffx/plugins/product_bundle/bisect/find_broken_links/find_broken_link.py",
                    )
                });

                if let Some(script_path) = script_path.filter(|p| p.exists()) {
                    match Command::new(&script_path)
                        .arg(pb_name)
                        .arg(from_success)
                        .arg(to_failure)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::inherit())
                        .spawn()
                    {
                        Ok(mut child) => {
                            if let Some(stdout) = child.stdout.take() {
                                let reader = BufReader::new(stdout);
                                for line in reader.lines().map_while(Result::ok) {
                                    print_fn(&line);
                                }
                            }
                            let _ = child.wait();
                        }
                        Err(err) => {
                            print_fn(&format!("Failed to spawn find_broken_link.py: {}", err));
                        }
                    }
                } else {
                    print_fn("Could not find find_broken_link.py script.");
                }
            }
            Err(e)
        }
    }
}

async fn get_pb_release_info<T: MOSClientTrait, PrintFn>(
    client: &mut T,
    name: &str,
    version: &str,
    print_fn: &mut PrintFn,
) -> Result<Vec<MOSIdentifier>>
where
    PrintFn: FnMut(&str),
{
    print_fn(&format!("-Retrieving release_info for {}@{}", name, version));
    let response = client
        .get_pb_release_info(name.to_string(), version.to_string())
        .await
        .context(format!(
            "*** Unknown MOS Identifier ***\n\nUnable to retrieve information about {}@{} from MOS. Perhaps this product bundle is not yet supported?\n\nFor more info, see go/fuchsia-product-bisection-userguide\n",
            name, version
        ))?;
    Ok(response)
}

async fn interpolate<T: MOSClientTrait, PrintFn>(
    client: &mut T,
    starting_artifacts: &[MOSIdentifier],
    ending_artifacts: &[MOSIdentifier],
    print_fn: &mut PrintFn,
) -> Result<SearchSpace>
where
    PrintFn: FnMut(&str),
{
    print_fn(" - Interpolating between versions");

    let mut dimensions = Vec::new();
    for (start, end) in starting_artifacts.iter().zip(ending_artifacts.iter()) {
        let dim = interpolate_artifact(client, start, end, print_fn).await?;
        dimensions.push(dim);
    }

    Ok(SearchSpace::new(dimensions))
}

async fn interpolate_artifact<T: MOSClientTrait, PrintFn>(
    client: &mut T,
    start: &MOSIdentifier,
    end: &MOSIdentifier,
    print_fn: &mut PrintFn,
) -> Result<Dimension>
where
    PrintFn: FnMut(&str),
{
    let versions = match client.interpolate(start, end).await {
        Ok(versions) => versions,
        Err(e) => {
            let original_message = extract_mos_error_message(&e);

            if original_message.contains("no common ancestor exists") {
                anyhow::bail!(
                    "*** No Common Ancestor ***\n\nThere is a missing link in the history chain for {} {} between {} and {}.\nWe unfortunately cannot fix this until b/462795427 is addressed.\nConsider shrinking the bisection range until this begins to work.\n\nFor more info, see go/fuchsia-product-bisection-userguide\n\nOriginal Message: {}\n",
                    start.artifact_type,
                    start.name,
                    start.version,
                    end.version,
                    original_message
                );
            } else if original_message.contains("accidentally swapped") {
                anyhow::bail!(
                    "*** Backwards Chain ***\n\nIn the history chain for {} {}, the from-success version ({}) appears _after_ the to-failure version ({}).\nPerhaps these arguments should be reversed?\n\nFor more info, see go/fuchsia-product-bisection-userguide\n\nOriginal Message: {}\n",
                    start.artifact_type,
                    start.name,
                    start.version,
                    end.version,
                    original_message
                );
            }
            return Err(e);
        }
    };
    print_fn(&format!("  - {} [{} releases]", start.id_no_version(), versions.len()));
    ensure!(
        versions.first() == Some(start),
        "Interpolated {} artifacts for '{}' do not start with the expected artifact. Expected: {}, Got: {}",
        start.artifact_type,
        start.name,
        start.id(),
        versions.first().map(|p| p.id()).as_deref().unwrap_or("<None>")
    );
    ensure!(
        versions.last() == Some(end),
        "Interpolated {} artifacts for '{}' do not end with the expected artifact. Expected: {}, Got: {}",
        end.artifact_type,
        end.name,
        end.id(),
        versions.last().map(|p| p.id()).as_deref().unwrap_or("<None>")
    );

    let version_strings: Vec<String> = versions.into_iter().map(|v| v.version).collect();
    Ok(Dimension::new(&start.name, start.artifact_type.clone(), &start.repository, version_strings))
}

#[derive(Deserialize)]
struct MOSErrorResponse {
    error: MOSErrorDetail,
}

#[derive(Deserialize)]
struct MOSErrorDetail {
    message: String,
}

fn extract_mos_error_message(e: &anyhow::Error) -> String {
    let err_str = format!("{:?}", e);
    if let Some(json_start) = err_str.find('{') {
        if let Ok(parsed) = serde_json::from_str::<MOSErrorResponse>(&err_str[json_start..]) {
            return parsed.error.message;
        }
    }
    format!("{}", e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_artifact_cache::ArtifactType;
    use std::fs::{self, File};
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    struct MockMOSClient {
        pb_release_info: Vec<MOSIdentifier>,
        interpolate_result: Result<Vec<MOSIdentifier>>,
    }

    #[async_trait(?Send)]
    impl MOSClientTrait for MockMOSClient {
        async fn get_pb_release_info(
            &mut self,
            _name: String,
            _version: String,
        ) -> Result<Vec<MOSIdentifier>> {
            Ok(self.pb_release_info.clone())
        }

        async fn interpolate(
            &self,
            _start: &MOSIdentifier,
            _end: &MOSIdentifier,
        ) -> Result<Vec<MOSIdentifier>> {
            match &self.interpolate_result {
                Ok(res) => Ok(res.clone()),
                Err(_) => anyhow::bail!("*** No Common Ancestor ***\nno common ancestor exists"),
            }
        }
    }

    #[test]
    fn test_get_search_space_success() {
        futures_lite::future::block_on(async move {
            let artifact = MOSIdentifier {
                name: "Platform".to_string(),
                artifact_type: ArtifactType::Platform,
                version: "1.0".to_string(),
                repository: "fuchsia".to_string(),
                cipd: None,
                slot: Slot::A,
            };
            let mut client = MockMOSClient {
                pb_release_info: vec![artifact.clone()],
                interpolate_result: Ok(vec![artifact.clone()]),
            };

            let mut output = Vec::new();
            let result =
                get_search_space(&mut client, "core.x64", "1.0", "2.0", None, Slot::A, |msg| {
                    output.push(msg.to_string())
                })
                .await;

            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_get_search_space_no_common_ancestor_no_fuchsia_dir() {
        futures_lite::future::block_on(async move {
            let artifact = MOSIdentifier {
                name: "Platform".to_string(),
                artifact_type: ArtifactType::Platform,
                version: "1.0".to_string(),
                repository: "fuchsia".to_string(),
                cipd: None,
                slot: Slot::A,
            };
            let mut client = MockMOSClient {
                pb_release_info: vec![artifact.clone()],
                interpolate_result: Err(anyhow::anyhow!("fail")),
            };

            let mut output = Vec::new();
            let result =
                get_search_space(&mut client, "core.x64", "1.0", "2.0", None, Slot::A, |msg| {
                    output.push(msg.to_string())
                })
                .await;

            assert!(result.is_err());
            assert!(
                output
                    .iter()
                    .any(|line| line.contains("Could not find find_broken_link.py script."))
            );
        });
    }

    #[test]
    fn test_get_search_space_no_common_ancestor_with_script() {
        futures_lite::future::block_on(async move {
            let temp_dir = tempdir().expect("tempdir");
            let script_dir = temp_dir
                .path()
                .join("vendor/google/scripts/ffx/plugins/product_bundle/bisect/find_broken_links");
            fs::create_dir_all(&script_dir).expect("create_dir_all");
            let script_path = script_dir.join("find_broken_link.py");
            {
                let mut file = File::create(&script_path).expect("create file");
                file.write_all(b"#!/bin/sh\necho \"Broken link found: commit abc1234\"\n")
                    .expect("write file");
            }
            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&script_path, perms).expect("set_permissions");
            }

            let temp_utf8_path = Utf8Path::from_path(temp_dir.path()).expect("utf8 path");

            let artifact = MOSIdentifier {
                name: "Platform".to_string(),
                artifact_type: ArtifactType::Platform,
                version: "1.0".to_string(),
                repository: "fuchsia".to_string(),
                cipd: None,
                slot: Slot::A,
            };
            let mut client = MockMOSClient {
                pb_release_info: vec![artifact.clone()],
                interpolate_result: Err(anyhow::anyhow!("fail")),
            };

            let mut output = Vec::new();
            let result = get_search_space(
                &mut client,
                "core.x64",
                "1.0",
                "2.0",
                Some(temp_utf8_path),
                Slot::A,
                |msg| output.push(msg.to_string()),
            )
            .await;

            assert!(result.is_err());
            assert!(
                !output
                    .iter()
                    .any(|line| line.contains("Could not find find_broken_link.py script."))
            );
            assert!(
                output.iter().any(|line| {
                    line.contains("Broken link found: commit abc1234")
                        || line.contains("Failed to spawn find_broken_link.py")
                }),
                "Expected script execution or spawn attempt, got: {:?}",
                output
            );
        });
    }

    #[test]
    fn test_get_search_space_no_common_ancestor_script_not_found() {
        futures_lite::future::block_on(async move {
            let temp_dir = tempdir().expect("tempdir");
            let temp_utf8_path = Utf8Path::from_path(temp_dir.path()).expect("utf8 path");

            let artifact = MOSIdentifier {
                name: "Platform".to_string(),
                artifact_type: ArtifactType::Platform,
                version: "1.0".to_string(),
                repository: "fuchsia".to_string(),
                cipd: None,
                slot: Slot::A,
            };
            let mut client = MockMOSClient {
                pb_release_info: vec![artifact.clone()],
                interpolate_result: Err(anyhow::anyhow!("fail")),
            };

            let mut output = Vec::new();
            let result = get_search_space(
                &mut client,
                "core.x64",
                "1.0",
                "2.0",
                Some(temp_utf8_path),
                Slot::A,
                |msg| output.push(msg.to_string()),
            )
            .await;

            assert!(result.is_err());
            assert!(
                output
                    .iter()
                    .any(|line| line.contains("Could not find find_broken_link.py script."))
            );
        });
    }

    #[test]
    fn test_extract_mos_error_message_valid_json() {
        let err_json = r#"{
            "error": {
                "message": "no common ancestor exists"
            }
        }"#;
        let err = anyhow::anyhow!("Some context: {}", err_json);
        let extracted = extract_mos_error_message(&err);
        assert_eq!(extracted, "no common ancestor exists");
    }

    #[test]
    fn test_extract_mos_error_message_invalid_json() {
        let err = anyhow::anyhow!("Some other error");
        let extracted = extract_mos_error_message(&err);
        assert_eq!(extracted, "Some other error");
    }

    #[test]
    fn test_extract_mos_error_message_json_missing_fields() {
        let err_json = r#"{
            "foo": "bar"
        }"#;
        let err = anyhow::anyhow!("Some context: {}", err_json);
        let extracted = extract_mos_error_message(&err);
        assert_eq!(extracted, format!("{}", err));
    }
}
