// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, bail};
use itertools::Itertools;
use serde::Deserialize;
use std::fs::{File, canonicalize};
use std::io::Read;
use std::path::Path;
use tar::Archive;
use tempfile::TempDir;

/// A Docker archive loaded from a tarball created by "docker save".
///
/// Its format is described here:
/// https://github.com/docker/docker-ce/blob/master/components/engine/image/spec/v1.2.md#combined-image-json--filesystem-changeset-format
pub struct DockerArchive {
    /// Where the tarball has been extracted.
    temp_dir: TempDir,

    /// The image's config data.
    config: Config,

    /// The names of the tar files within `temp_dir` containing each layer.
    layers: Vec<String>,
}

#[derive(Clone, Copy, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum DockerArchiveArchitecture {
    Amd64,
    Arm64,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct ManifestEntry {
    config: String,
    layers: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Config {
    architecture: DockerArchiveArchitecture,
    config: ConfigInner,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct ConfigInner {
    env: Vec<String>,
    cmd: Vec<String>,
}

/// Opens a `file` contained in `dir`. The file must be either a regular file or a symlink whose
/// target is also contained in the directory.
fn safe_open_in_dir(dir: &Path, file: &str) -> Result<File> {
    let canonicalized_dir = canonicalize(dir)?;
    let canonicalized_file = canonicalize(dir.join(file))?;
    if canonicalized_file.starts_with(&canonicalized_dir) {
        if canonicalized_file.is_file() {
            Ok(File::open(canonicalized_file)?)
        } else {
            bail!(
                "Blocked attempt to open {} which is not a regular file",
                canonicalized_file.display()
            );
        }
    } else {
        bail!(
            "Blocked attempt to open {} outside {}",
            canonicalized_file.display(),
            canonicalized_dir.display()
        );
    }
}

fn read_json_file<T: serde::de::DeserializeOwned>(mut file: File) -> Result<T> {
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(serde_json::from_slice::<T>(&data)?)
}

impl DockerArchive {
    /// Opens the given Docker archive.
    pub fn open(input_archive: impl Read) -> Result<DockerArchive> {
        // Extract the tarball into a temporary directory.
        let temp_dir = TempDir::new()?;
        Archive::new(input_archive).unpack(temp_dir.path())?;

        // Parse the manifest.
        let manifest_file = safe_open_in_dir(temp_dir.path(), "manifest.json")?;
        let manifest: Vec<ManifestEntry> = read_json_file(manifest_file)?;
        let Ok(Some(manifest)) = manifest.into_iter().at_most_one() else {
            bail!("Manifest should contain exactly one entry");
        };

        // Parse the config.
        let config_file = safe_open_in_dir(temp_dir.path(), &manifest.config)?;
        let config = read_json_file::<Config>(config_file)?;

        Ok(DockerArchive { temp_dir, config, layers: manifest.layers })
    }

    /// Returns an iterator that yields all the layers.
    pub fn layers(&self) -> Result<impl Iterator<Item = Archive<File>>> {
        let result: Result<Vec<_>> = self
            .layers
            .iter()
            .map(|layer| Ok(Archive::new(safe_open_in_dir(self.temp_dir.path(), layer)?)))
            .collect();
        Ok(result?.into_iter())
    }

    /// Returns the container's target architecture.
    pub fn architecture(&self) -> DockerArchiveArchitecture {
        self.config.architecture
    }

    /// Returns the defined environment variables (in "KEY=VALUE" format).
    pub fn environ(&self) -> Vec<&str> {
        self.config.config.env.iter().map(String::as_str).collect()
    }

    /// Returns the image-provided default command.
    pub fn default_command(&self) -> Vec<&str> {
        self.config.config.cmd.iter().map(String::as_str).collect()
    }
}
