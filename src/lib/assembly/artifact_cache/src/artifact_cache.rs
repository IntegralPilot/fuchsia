// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::artifact::{Artifact, CIPDPackage};
use crate::{build_api, cipd, mos};

use anyhow::{Context, Result, bail};
use assembly_config_schema::Architecture;
use camino::Utf8PathBuf;
use gcs::client::Client;
use std::str::FromStr;
use thiserror::Error;

/// Cache of assembly artifacts that may be downloaded from CIPD.
pub struct ArtifactCache {
    build_dir: Option<Utf8PathBuf>,
    cache: Utf8PathBuf,
    ensured_artifacts: Utf8PathBuf,
    gcs_client: Client,
}

impl ArtifactCache {
    /// Construct a new ArtifactCache.
    pub fn new(build_dir: Option<Utf8PathBuf>, gcs_client: Client) -> Result<Self> {
        let home = std::env::home_dir().unwrap();
        let home = Utf8PathBuf::from_path_buf(home).unwrap();
        let cache = home.join(".fuchsia").join("cipd");
        let ensured_artifacts = cache.join("artifacts");
        Ok(Self { build_dir, cache, ensured_artifacts, gcs_client })
    }

    /// Returns a reference to the underlying GCS client.
    pub fn gcs_client(&self) -> &Client {
        &self.gcs_client
    }

    /// Delete all ensured artifacts.
    pub fn purge(&self) -> Result<()> {
        if self.ensured_artifacts.exists() {
            std::fs::remove_dir_all(&self.ensured_artifacts).context("Purging CIPD packages")?;
        }
        Ok(())
    }

    /// Resolve a product to a local path, downloading it if necessary.
    pub async fn resolve_product(
        &self,
        product_config: String,
    ) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_artifact_with_suggestion(
            product_config,
            "products.json",
            "product",
            &["fuchsia/assembly/products", "fuchsia_internal/assembly/products"],
        )
        .await
    }

    /// Resolve a board to a local path, downloading it if necessary.
    pub async fn resolve_board(&self, board_config: String) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_artifact_with_suggestion(
            board_config,
            "boards.json",
            "board",
            &["fuchsia/assembly/boards", "fuchsia_internal/assembly/boards"],
        )
        .await
    }

    async fn resolve_artifact_with_suggestion(
        &self,
        artifact: String,
        build_api: &str,
        artifact_type: &str,
        cipd_dirs: &[&str],
    ) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_product_or_board_internal(artifact, build_api).await.map_err(|e| {
            let suggested_tags = match self.suggest_product_or_board(build_api, cipd_dirs) {
                Ok(tags) => tags,
                Err(e) => return ArtifactError::new(e),
            };
            let suggestion = format!(
                "Use a known {}:{}",
                artifact_type,
                suggested_tags
                    .iter()
                    .map(|p| format!("\n  {}", p))
                    .collect::<Vec<String>>()
                    .concat()
            );
            ArtifactError::with_suggestion(e.error, suggestion)
        })
    }

    async fn resolve_product_or_board_internal(
        &self,
        artifact: String,
        build_api: &str,
    ) -> Result<Utf8PathBuf, ArtifactError> {
        if let Some(artifact) = cipd::parse_cipd_artifact(&artifact)? {
            return self.resolve(&artifact).await;
        }

        if let Some(artifact) = mos::parse_mos_artifact(&artifact)? {
            return self.resolve(&artifact).await;
        }

        if let Some(artifact) = parse_local_path(&artifact)? {
            return self.resolve(&artifact).await;
        }

        self.resolve(&build_api::parse_local_artifact(
            &artifact,
            self.build_dir.as_ref(),
            build_api,
        )?)
        .await
    }

    /// Resolve a PIB to a local path, downloading it if necessary.
    pub async fn resolve_pib(&self, pib: String) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_artifact_with_suggestion(
            pib,
            "product_input_bundles.json",
            "product_input_bundle",
            &[
                "fuchsia/assembly/product_input_bundles",
                "fuchsia_internal/assembly/product_input_bundles",
            ],
        )
        .await
    }

    /// Resolve a board to a local path, downloading it if necessary.
    pub async fn resolve_bib_set(&self, bib_set: String) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_artifact_with_suggestion(
            bib_set,
            "board_input_bundle_sets.json",
            "board_input_bundle_set",
            &[
                "fuchsia/assembly/board_input_bundles",
                "fuchsia_internal/assembly/board_input_bundles",
            ],
        )
        .await
    }

    /// Resolve a platform to a local path, downloading it if necessary.
    pub async fn resolve_platform(
        &self,
        platform: Option<String>,
        arch: &Architecture,
    ) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_platform_with_suggestion(platform, arch).await
    }

    async fn resolve_platform_with_suggestion(
        &self,
        platform: Option<String>,
        arch: &Architecture,
    ) -> Result<Utf8PathBuf, ArtifactError> {
        self.resolve_platform_internal(platform, arch).await.map_err(|e| {
            let suggested_tags = match self.suggest_platform(arch) {
                Ok(tags) => tags,
                Err(e) => return ArtifactError::new(e),
            };
            let suggestion = format!(
                "Use a known platform:{}",
                suggested_tags
                    .iter()
                    .map(|p| format!("\n  {}", p))
                    .collect::<Vec<String>>()
                    .concat()
            );
            ArtifactError::with_suggestion(e.error, suggestion)
        })
    }

    async fn resolve_platform_internal(
        &self,
        platform: Option<String>,
        arch: &Architecture,
    ) -> Result<Utf8PathBuf, ArtifactError> {
        let platform = match platform {
            None => {
                let platform_artifact = build_api::get_default_platform(self.build_dir.as_ref())?;
                return self.resolve(&platform_artifact).await;
            }
            Some(platform) => platform,
        };

        if let Some(artifact) = cipd::parse_cipd_artifact(&platform)? {
            return self.resolve(&artifact).await;
        }

        if let Some(artifact) = mos::parse_mos_artifact(&platform)? {
            return self.resolve(&artifact).await;
        }

        if let Some(artifact) = parse_local_path(&platform)? {
            return self.resolve(&artifact).await;
        }

        // Assume the input is a tag to the default CIPD location.
        self.resolve(&cipd::get_default_platform(&platform, arch)).await
    }

    /// Retrieve a list of suggested product or board artifacts.
    fn suggest_product_or_board(&self, build_api: &str, cipd_dirs: &[&str]) -> Result<Vec<String>> {
        let mut artifacts = build_api::suggest_local_artifacts(self.build_dir.as_ref(), build_api)?;
        for cipd_dir in cipd_dirs {
            let cipd_artifacts = cipd::list_packages(cipd_dir)?;
            artifacts.extend(cipd_artifacts.into_iter().map(|artifact| {
                CIPDPackage { path: artifact.into(), tag: "latest".to_string() }.to_string()
            }));
        }
        Ok(artifacts)
    }

    /// Retrieve a list of suggested platform artifacts.
    pub fn suggest_platform(&self, arch: &Architecture) -> Result<Vec<String>> {
        let mut suggestions = vec![
            "<omitted>: use local default".to_string(),
            "latest:    use latest in CIPD".to_string(),
        ];
        let cipd_suggestions =
            cipd::list_recent_package_instances(format!("fuchsia/assembly/platform/{}", arch))?;
        suggestions.extend(cipd_suggestions);
        Ok(suggestions)
    }

    /// Resolve an artifact to a local path, downloading it if necessary.
    async fn resolve(&self, artifact: &Artifact) -> Result<Utf8PathBuf, ArtifactError> {
        match artifact {
            Artifact::Local(path) => Ok(path.clone()),
            Artifact::CIPD(package) => {
                let destination = self.ensured_artifacts.join(&package.path);
                cipd::download(package, &destination, &self.cache)?;
                Ok(destination)
            }
            Artifact::MOS(identifier) => {
                let gcs_client = &self.gcs_client;
                let package = mos::get_cipd_package_from_mos_artifact(&identifier, gcs_client)
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(format!("Unable to retrieve [{}].", identifier.id()))
                    })?;
                let destination = self.ensured_artifacts.join(package.path.clone());
                cipd::download(&package, &destination, &self.cache)?;
                Ok(destination)
            }
        }
    }
}

/// Find an artifact from a local path.
fn parse_local_path(s: impl AsRef<str>) -> Result<Option<Artifact>> {
    let s = s.as_ref();
    let exists = std::fs::metadata(s).is_ok();

    // If the input does not have a slash, it might be a name, not a path.
    // We do not want to return an error in that case.
    if !exists && !s.contains('/') {
        return Ok(None);
    }

    if !exists {
        bail!(
            "The input looks like a path because it has a '/', but the path does not exist: {}",
            s
        );
    }

    let path =
        Utf8PathBuf::from_str(s).with_context(|| format!("Local path is not utf8: {}", s))?;
    Ok(Some(Artifact::Local(path)))
}

/// An error when attempting to resolve an artifact.
#[derive(Debug, Error)]
#[error("{error}")]
pub struct ArtifactError {
    /// The underlying error.
    #[source]
    pub error: anyhow::Error,

    /// A suggestion to fix the error.
    pub suggestion: Option<String>,
}

impl ArtifactError {
    /// Construct a new ArtifactError without a suggestion.
    pub fn new(error: anyhow::Error) -> Self {
        Self { error, suggestion: None }
    }

    /// Construct a new ArtifactError with a suggestion.
    pub fn with_suggestion(error: anyhow::Error, suggestion: String) -> Self {
        Self { error, suggestion: Some(suggestion) }
    }
}

impl From<anyhow::Error> for ArtifactError {
    fn from(e: anyhow::Error) -> Self {
        Self::new(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_config_schema::Architecture;
    use std::fs::File;
    use tempfile::tempdir;

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_resolve_board_by_name() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        let boards_json = tmp_path.join("boards.json");
        let boards_file = File::create(&boards_json).unwrap();
        let boards = serde_json::json!([
            {
                "name": "board_a",
                "outdir": "path/to/local/board",
                "label": "//build/images/fuchsia:fuchsia"
            }
        ]);
        serde_json::to_writer(&boards_file, &boards).unwrap();
        let board_path = tmp_path.join("path/to/local/board");
        std::fs::create_dir_all(board_path.parent().unwrap()).unwrap();
        File::create(&board_path).unwrap();

        let cache =
            ArtifactCache::new(Some(tmp_path.clone()), gcs::client::Client::initial().unwrap())
                .unwrap();
        let resolved_path = cache.resolve_board("board_a".to_string()).await.unwrap();
        assert_eq!(resolved_path, board_path);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_resolve_board_by_path() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let board_path = tmp_path.join("board");
        File::create(&board_path).unwrap();

        let cache =
            ArtifactCache::new(Some(tmp_path.clone()), gcs::client::Client::initial().unwrap())
                .unwrap();
        let resolved_path = cache.resolve_board(board_path.to_string()).await.unwrap();
        assert_eq!(resolved_path, board_path);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_resolve_platform_default() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        let platform_artifacts_json = tmp_path.join("platform_artifacts.json");
        let mut platform_artifacts_file = File::create(&platform_artifacts_json).unwrap();
        serde_json::to_writer(
            &mut platform_artifacts_file,
            &serde_json::json!([
                {
                    "path": "path/to/local/platform",
                }
            ]),
        )
        .unwrap();
        let platform_path = tmp_path.join("path/to/local/platform");
        std::fs::create_dir_all(platform_path.parent().unwrap()).unwrap();
        File::create(&platform_path).unwrap();

        let cache =
            ArtifactCache::new(Some(tmp_path.clone()), gcs::client::Client::initial().unwrap())
                .unwrap();
        let resolved_path = cache.resolve_platform(None, &Architecture::X64).await.unwrap();
        assert_eq!(resolved_path, platform_path);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_resolve_platform_by_path() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let platform_path = tmp_path.join("platform");
        File::create(&platform_path).unwrap();

        let cache =
            ArtifactCache::new(Some(tmp_path.clone()), gcs::client::Client::initial().unwrap())
                .unwrap();
        let resolved_path = cache
            .resolve_platform(Some(platform_path.to_string()), &Architecture::X64)
            .await
            .unwrap();
        assert_eq!(resolved_path, platform_path);
    }
}
