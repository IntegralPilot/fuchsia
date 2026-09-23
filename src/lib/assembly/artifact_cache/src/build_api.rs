// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::artifact::Artifact;
use crate::artifact_cache::ArtifactError;
use crate::gn_label::GNLabel;

use anyhow::{Context, Result, anyhow, bail};
use camino::Utf8PathBuf;
use serde::Deserialize;

#[derive(Deserialize, Clone)]
struct BuildApiEntry {
    name: String,
    outdir: Utf8PathBuf,
    label: GNLabel,
}

#[derive(Deserialize)]
struct PlatformBuildApiEntry {
    path: Utf8PathBuf,
}

/// Find an artifact with `name` in the Fuchsia `build_dir` using the `build_api`.
pub fn parse_local_artifact(
    name: impl AsRef<str>,
    build_dir: Option<&Utf8PathBuf>,
    build_api: impl AsRef<str>,
) -> Result<Artifact> {
    let build_dir = match build_dir {
        None => bail!("identifying artifacts by their name can only be done in a fuchsia checkout"),
        Some(build_dir) => build_dir,
    };
    let build_api_entries = read_build_api(build_dir, build_api)?;
    let artifact_entry = build_api_entries
        .iter()
        .find(|b| b.name == name.as_ref())
        .with_context(|| format!("searching for artifact '{}'", name.as_ref()))?;
    let artifact_path = build_dir.join(&artifact_entry.outdir);

    // If the artifact does not exist, then prompt the user to build it.
    if !std::fs::metadata(&artifact_path).is_ok() {
        return Err(anyhow!(
            "Build it with: fx build {}",
            artifact_entry.label.without_toolchain()
        ))
        .with_context(|| {
            format!("'{}' is found in the build api, but needs to be built.", name.as_ref(),)
        });
    }

    Ok(Artifact::Local(artifact_path))
}

pub fn suggest_local_artifacts(
    build_dir: Option<&Utf8PathBuf>,
    build_api: impl AsRef<str>,
) -> Result<Vec<String>> {
    let build_dir = match build_dir {
        // We should never fail to provide suggestions if we are outside a
        // fuchsia checkout.
        None => return Ok(vec![]),
        Some(build_dir) => build_dir,
    };
    let build_api_entries = crate::build_api::read_build_api(build_dir, build_api)?;
    Ok(build_api_entries.into_iter().map(|b| b.name).collect::<Vec<_>>())
}

pub fn get_default_platform(build_dir: Option<&Utf8PathBuf>) -> Result<Artifact, ArtifactError> {
    let build_dir = build_dir
        .as_ref()
        .context("could not find environment variable BUILD_DIR")
        .context("--platform is required outside a fuchsia checkout")?;
    let build_api = build_dir.join("platform_artifacts.json");
    let build_api_file =
        std::fs::File::open(&build_api).with_context(|| format!("Opening: {}", build_api))?;
    let platform_artifacts: Vec<PlatformBuildApiEntry> = serde_json::from_reader(build_api_file)
        .with_context(|| format!("Parsing: {}", build_api))?;
    let platform_artifacts = platform_artifacts
        .first()
        .context("searching for platform artifacts")
        .with_context(|| format!("searching build api: {}", build_api))?;
    let platform_artifacts_path = build_dir.join(&platform_artifacts.path);
    Ok(Artifact::Local(platform_artifacts_path))
}

fn read_build_api(
    build_dir: &Utf8PathBuf,
    build_api: impl AsRef<str>,
) -> Result<Vec<BuildApiEntry>> {
    let build_api_path = build_dir.join(build_api.as_ref());
    let build_api_file = std::fs::File::open(&build_api_path)
        .with_context(|| format!("Opening: {}", build_api_path))?;
    let build_api_entries: Vec<BuildApiEntry> = serde_json::from_reader(build_api_file)
        .with_context(|| format!("Parsing: {}", build_api_path))?;
    Ok(build_api_entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, create_dir};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_parse_local_artifact_success() {
        let dir = tempdir().unwrap();
        let build_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let build_api_path = build_dir.join("build_api.json");
        let artifact_path = build_dir.join("artifact");

        let mut build_api_file = File::create(&build_api_path).unwrap();
        write!(
            build_api_file,
            r#"[
                {{
                    "name": "my-artifact",
                    "outdir": "artifact",
                    "label": "//:my-artifact"
                }}
            ]"#
        )
        .unwrap();

        File::create(&artifact_path).unwrap();

        let artifact =
            parse_local_artifact("my-artifact", Some(&build_dir), "build_api.json").unwrap();
        assert_eq!(artifact, Artifact::Local(artifact_path));
    }

    #[test]
    fn test_parse_local_artifact_not_built() {
        let dir = tempdir().unwrap();
        let build_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let build_api_path = build_dir.join("build_api.json");

        let mut build_api_file = File::create(&build_api_path).unwrap();
        write!(
            build_api_file,
            r#"[
                {{
                    "name": "my-artifact",
                    "outdir": "artifact",
                    "label": "//:my-artifact"
                }}
            ]"#
        )
        .unwrap();

        let err =
            parse_local_artifact("my-artifact", Some(&build_dir), "build_api.json").unwrap_err();
        assert!(
            err.to_string()
                .contains("'my-artifact' is found in the build api, but needs to be built.")
        );
    }

    #[test]
    fn test_parse_local_artifact_not_found() {
        let dir = tempdir().unwrap();
        let build_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let build_api_path = build_dir.join("build_api.json");

        let mut build_api_file = File::create(&build_api_path).unwrap();
        write!(build_api_file, "[]").unwrap();

        let err =
            parse_local_artifact("my-artifact", Some(&build_dir), "build_api.json").unwrap_err();
        assert!(err.to_string().contains("searching for artifact 'my-artifact'"));
    }

    #[test]
    fn test_parse_local_artifact_no_build_dir() {
        let err = parse_local_artifact("my-artifact", None, "build_api.json").unwrap_err();
        assert!(err.to_string().contains(
            "identifying artifacts by their name can only be done in a fuchsia checkout"
        ));
    }

    #[test]
    fn test_suggest_local_artifacts_success() {
        let dir = tempdir().unwrap();
        let build_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let build_api_path = build_dir.join("build_api.json");

        let mut build_api_file = File::create(&build_api_path).unwrap();
        write!(
            build_api_file,
            r#"[
                {{
                    "name": "my-artifact",
                    "outdir": "artifact",
                    "label": "//:my-artifact"
                }},
                {{
                    "name": "another-artifact",
                    "outdir": "artifact2",
                    "label": "//:another-artifact"
                }}
            ]"#
        )
        .unwrap();

        let suggestions = suggest_local_artifacts(Some(&build_dir), "build_api.json").unwrap();
        assert_eq!(suggestions, vec!["my-artifact", "another-artifact"]);
    }

    #[test]
    fn test_suggest_local_artifacts_no_build_dir() {
        let suggestions = suggest_local_artifacts(None, "build_api.json").unwrap();
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_get_default_platform_success() {
        let dir = tempdir().unwrap();
        let build_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let platform_artifacts_path = build_dir.join("platform_artifacts.json");
        let artifact_path = build_dir.join("platform/artifact");
        create_dir(build_dir.join("platform")).unwrap();
        File::create(&artifact_path).unwrap();

        let mut platform_artifacts_file = File::create(&platform_artifacts_path).unwrap();
        write!(
            platform_artifacts_file,
            r#"[
                {{
                    "path": "platform/artifact"
                }}
            ]"#
        )
        .unwrap();

        let artifact = get_default_platform(Some(&build_dir)).unwrap();
        assert_eq!(artifact, Artifact::Local(artifact_path));
    }

    #[test]
    fn test_get_default_platform_no_build_dir() {
        let err = get_default_platform(None).unwrap_err();
        assert!(err.to_string().contains("--platform is required outside a fuchsia checkout"));
    }

    #[test]
    fn test_get_default_platform_missing_file() {
        let dir = tempdir().unwrap();
        let build_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let err = get_default_platform(Some(&build_dir)).unwrap_err();
        assert!(err.to_string().contains("Opening:"));
    }
}
