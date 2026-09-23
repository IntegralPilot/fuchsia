// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ArtifactError;
use crate::artifact::{Artifact, CIPDPackage};

use anyhow::{Context, Result, anyhow, bail};
use assembly_config_schema::Architecture;
use camino::Utf8PathBuf;
use serde::Deserialize;
use std::process::{Command, Stdio};
use std::str::FromStr;
use tempfile::tempdir;

#[derive(Debug, Deserialize)]
struct Pin {
    instance_id: String,
}

#[derive(Debug, Deserialize)]
struct Instance {
    pin: Pin,
}

#[derive(Debug, Deserialize)]
struct InstancesResult {
    instances: Vec<Instance>,
}

#[derive(Debug, Deserialize)]
struct Instances {
    result: InstancesResult,
}

#[derive(Debug, Deserialize)]
struct Tag {
    tag: String,
}

#[derive(Debug, Deserialize)]
struct DescribeResult {
    tags: Vec<Tag>,
}

#[derive(Debug, Deserialize)]
struct Describe {
    result: DescribeResult,
}

#[derive(Debug, Deserialize)]
struct Ls {
    result: Option<Vec<String>>,
}

type CipdCommand = dyn Fn(&str, &[&str]) -> Result<std::process::Output>;

/// Find an artifact in CIPD.
pub fn parse_cipd_artifact(s: impl AsRef<str>) -> Result<Option<Artifact>> {
    let s = s.as_ref();
    if let Some(cipd_path_and_version) = s.strip_prefix("cipd://") {
        let (path, version) = cipd_path_and_version
            .split_once("@")
            .with_context(|| format!("Add the version using a CIPD tag: {}@<cipd-tag>", s))
            .with_context(|| format!("Artifact is missing a version: {}", s))?;
        if version.is_empty() {
            bail!("Version tag must not be empty. Use format cipd://<package>@<version-tag>");
        }
        let path = Utf8PathBuf::from_str(path)
            .with_context(|| format!("Artifact path is not utf8: {}", s))?;
        let tag = version_to_cipd_tag(version);
        Ok(Some(Artifact::CIPD(CIPDPackage { path, tag })))
    } else {
        Ok(None)
    }
}

pub fn list_packages(directory: impl AsRef<str>) -> Result<Vec<String>> {
    list_packages_with_cipd(directory, &cipd)
}

/// Accepts a CipdCommand so that it can be mocked for tests.
fn list_packages_with_cipd(
    directory: impl AsRef<str>,
    cipd_command: &CipdCommand,
) -> Result<Vec<String>> {
    let temp_dir = tempdir()?;
    let ls_path = temp_dir.path().join("ls.json");

    let output = (cipd_command)(
        "cipd",
        &["ls", directory.as_ref(), "-json-output", ls_path.to_str().unwrap()],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Not all users have access to fuchsia_internal. If they do not, then
        // cipd will exit with an error. We can ignore this error and instead
        // return an empty list of packages.
        if stderr.contains("No matching packages") {
            return Ok(Vec::new());
        }
        anyhow::bail!("Failed to list packages from CIPD: {}", stderr);
    }

    let ls_file = std::fs::File::open(&ls_path)
        .with_context(|| format!("Failed to open ls file: {:?}", ls_path))?;
    let ls: Ls = serde_json::from_reader(ls_file).context("Failed to parse ls JSON from CIPD")?;

    let packages =
        ls.result.unwrap_or_default().into_iter().filter(|s| !s.ends_with('/')).collect();

    Ok(packages)
}

pub fn list_recent_package_instances(package: impl AsRef<str>) -> Result<Vec<String>> {
    list_shortest_tags_for_package(package, &cipd)
}

fn cipd(name: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new(name)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context(format!("Failed to execute command: {}", name))?;
    Ok(output)
}

fn list_shortest_tags_for_package(
    package: impl AsRef<str>,
    cipd_command: &CipdCommand,
) -> Result<Vec<String>> {
    let temp_dir = tempdir()?;
    let instances_path = temp_dir.path().join("instances.json");

    // Get the list of instances.
    let instances_output = (cipd_command)(
        "cipd",
        &[
            "instances",
            package.as_ref(),
            "-limit",
            "5",
            "-json-output",
            instances_path.to_str().unwrap(),
        ],
    )?;
    if !instances_output.status.success() {
        anyhow::bail!(
            "Failed to get instances from CIPD: {}",
            String::from_utf8_lossy(&instances_output.stderr)
        );
    }

    let instances_file = std::fs::File::open(&instances_path)
        .context(format!("Failed to open instances file: {:?}", instances_path))?;
    let instances: Instances = serde_json::from_reader(instances_file)
        .context("Failed to parse instances JSON from CIPD")?;

    // For each instance, get the tags.
    let mut tags = Vec::new();
    for instance in instances.result.instances {
        let instance_path = temp_dir.path().join("instance.json");
        let describe_output = (cipd_command)(
            "cipd",
            &[
                "describe",
                package.as_ref(),
                "-version",
                &instance.pin.instance_id,
                "-json-output",
                instance_path.to_str().unwrap(),
            ],
        )?;
        if !describe_output.status.success() {
            anyhow::bail!(
                "Failed to describe instance from CIPD: {}",
                String::from_utf8_lossy(&describe_output.stderr)
            );
        }

        let instance_file = std::fs::File::open(&instance_path)
            .context(format!("Failed to open instance file: {:?}", instance_path))?;
        let describe: Describe = serde_json::from_reader(instance_file)
            .context("Failed to parse instance JSON from CIPD")?;
        if let Some(shortest_tag) =
            describe.result.tags.into_iter().map(|t| t.tag).min_by_key(|t| t.len())
        {
            tags.push(shortest_tag);
        }
    }
    Ok(tags)
}

/// Download an artifact from CIPD to `destination`.
pub fn download(
    package: &CIPDPackage,
    destination: &Utf8PathBuf,
    cipd_cache_dir: &Utf8PathBuf,
) -> Result<(), ArtifactError> {
    // Prepare the output directory.
    let artifact_name = destination
        .file_name()
        .with_context(|| format!("Artifact path does not have a file name: {}", destination))?
        .to_string();
    let artifact_dir = destination
        .parent()
        .with_context(|| format!("Artifact path does not have a parent: {}", destination))?;
    std::fs::create_dir_all(&artifact_dir).map_err(|e| anyhow!(e))?;

    // Write the ensure file.
    let ensure_contents = format!("{} {}", package.path, package.tag);
    let ensure_path = artifact_dir.join(format!("{}.ensure", artifact_name));
    std::fs::write(&ensure_path, &ensure_contents).map_err(|e| anyhow!(e))?;

    println!("Downloading: {}", package);

    // Download from CIPD.
    let child = Command::new("cipd")
        .arg("ensure")
        .arg("-ensure-file")
        .arg(&ensure_path)
        .arg("-root")
        .arg(&destination)
        .arg("-cache-dir")
        .arg(&cipd_cache_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to execute cipd command")?;
    let child_output = child.wait_with_output().context("Waiting for cipd to finish")?;
    if !child_output.status.success() {
        return Err(
            anyhow!("Failed to download CIPD package: {}@{}", package.path, package.tag).into()
        );
    }
    return Ok(());
}

pub fn get_default_platform(version: impl AsRef<str>, arch: &Architecture) -> Artifact {
    let path = Utf8PathBuf::from_str("fuchsia/assembly/platform").unwrap();
    let path = path.join(arch.to_string());
    let tag = version_to_cipd_tag(version);
    Artifact::CIPD(CIPDPackage { path, tag })
}

fn version_to_cipd_tag(version: impl AsRef<str>) -> String {
    if version.as_ref() == "latest" {
        return "latest".into();
    }
    if version.as_ref().starts_with("version:") {
        return version.as_ref().to_string();
    }
    return format!("version:{}", version.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;

    const LS_JSON: &str = r#"
    {
      "result": [
        "fuchsia/assembly/boards/arm64",
        "fuchsia/assembly/boards/x64",
        "fuchsia/assembly/boards/some_dir/"
      ]
    }
    "#;

    const INSTANCES_JSON: &str = r#"
    {
      "result": {
        "instances": [
          {
            "pin": {
              "package": "fuchsia/assembly/platform/arm64",
              "instance_id": "cM7nGnWDdjQIlF2AP1udpTTiSBCpcW0Sua4y3kxFN3EC"
            }
          },
          {
            "pin": {
              "package": "fuchsia/assembly/platform/arm64",
              "instance_id": "another-instance-id"
            }
          }
        ]
      }
    }
    "#;

    const DESCRIBE_JSON: &str = r#"
    {
      "result": {
        "tags": [
          {
            "tag": "git_revision:ef30ae17fceac84102dc15574ced102ba2e4d3d7"
          },
          {
            "tag": "version:29.20250813.3.1"
          }
        ]
      }
    }
    "#;

    const DESCRIBE_JSON_2: &str = r#"
    {
      "result": {
        "tags": [
          {
            "tag": "version:29.20250813.3.2"
          },
          {
            "tag": "a-very-long-git-revision-that-is-longer-than-the-version"
          }
        ]
      }
    }
    "#;

    fn mock_cipd(responses: Vec<String>) -> Box<CipdCommand> {
        let mock_responses = RefCell::new(VecDeque::from(responses));
        Box::new(move |_name, args| {
            let json_path = args.iter().rfind(|a| a.ends_with(".json")).unwrap();
            let response = mock_responses.borrow_mut().pop_front().unwrap();
            std::fs::write(Path::new(json_path), response).unwrap();
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        })
    }

    #[test]
    fn test_list_packages() {
        let packages = list_packages_with_cipd(
            "fuchsia/assembly/boards",
            &mock_cipd(vec![LS_JSON.to_string()]),
        )
        .unwrap();

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0], "fuchsia/assembly/boards/arm64");
        assert_eq!(packages[1], "fuchsia/assembly/boards/x64");
    }

    #[test]
    fn test_list() {
        let tags = list_shortest_tags_for_package(
            "fake/package",
            &mock_cipd(vec![
                INSTANCES_JSON.to_string(),
                DESCRIBE_JSON.to_string(),
                DESCRIBE_JSON_2.to_string(),
            ]),
        )
        .unwrap();

        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], "version:29.20250813.3.1");
        assert_eq!(tags[1], "version:29.20250813.3.2");
    }

    #[test]
    fn test_version_to_cipd_tag() {
        assert_eq!(version_to_cipd_tag("latest"), "latest");
        assert_eq!(version_to_cipd_tag("version:1234"), "version:1234");
        assert_eq!(version_to_cipd_tag("1234"), "version:1234");
    }

    #[test]
    fn test_parse_cipd_valid() {
        let artifact = parse_cipd_artifact("cipd://path/to/package@1.2.3").unwrap().unwrap();
        assert_eq!(
            artifact,
            Artifact::CIPD(CIPDPackage {
                path: "path/to/package".into(),
                tag: "version:1.2.3".into()
            })
        );
    }

    #[test]
    fn test_parse_cipd_latest() {
        let artifact = parse_cipd_artifact("cipd://path/to/package@latest").unwrap().unwrap();
        assert_eq!(
            artifact,
            Artifact::CIPD(CIPDPackage { path: "path/to/package".into(), tag: "latest".into() })
        );
    }

    #[test]
    fn test_parse_cipd_missing_version() {
        let result = parse_cipd_artifact("cipd://path/to/package");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_cipd_empty_version() {
        let result = parse_cipd_artifact("cipd://path/to/package@");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_cipd_not_cipd() {
        let result = parse_cipd_artifact("file://path/to/file").unwrap();
        assert_eq!(result, None);
    }
}
