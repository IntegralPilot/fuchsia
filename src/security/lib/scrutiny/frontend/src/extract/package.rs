// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use fuchsia_archive::Utf8Reader as FarReader;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use scrutiny_collection::core::Packages;
use scrutiny_collection::model::DataModel;
use scrutiny_utils::artifact::{ArtifactReader, FileArtifactReader};
use serde_json::json;
use serde_json::value::Value;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn create_dir_all<P: AsRef<Path> + ?Sized>(path: &P) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|err| anyhow!("Failed to create directory {}: {}", path.as_ref().display(), err))
}

fn create_file<P>(path: &P) -> Result<fs::File>
where
    P: AsRef<Path>,
{
    File::create(path)
        .map_err(|err| anyhow!("Failed to create file at {}: {}", path.as_ref().display(), err))
}

fn write_file<P>(path: &P, file: &mut fs::File, buf: &mut [u8]) -> Result<()>
where
    P: AsRef<Path>,
{
    file.write_all(buf)
        .map_err(|err| anyhow!("Failed to write file at {}: {}", path.as_ref().display(), err))
}

pub struct PackageExtractController {}

impl PackageExtractController {
    pub fn extract(
        model: Arc<DataModel>,
        url: AbsolutePackageUrl,
        output: impl AsRef<Path>,
    ) -> Result<Value> {
        let output = output.as_ref();
        let mut artifact_reader = FileArtifactReader::new(
            &PathBuf::new(),
            &model.config().blobs_directory(),
            model.config().delivery_blob_type,
        );
        let packages = &model.get::<Packages>()?.entries;
        for package in packages.iter() {
            if package.matches_url(&url) {
                create_dir_all(&output).context("Failed to create output directory")?;

                let merkle_string = format!("{}", package.merkle);
                let blob = artifact_reader
                    .open(Path::new(&merkle_string))
                    .context("Failed to read package from blobfs archive(s)")?;
                let mut far = FarReader::new(blob)?;

                let pkg_files: Vec<String> = far.list().map(|e| e.path().to_string()).collect();
                // Extract all the far meta files.
                for file_name in pkg_files.iter() {
                    let mut data = far.read_file(file_name)?;
                    let file_path = output.join(file_name);
                    if let Some(parent_dir) = file_path.as_path().parent() {
                        create_dir_all(parent_dir)
                            .context("Failed to create far meta directory")?;
                    }
                    let mut package_file =
                        create_file(&file_path).context("Failed to create package file")?;
                    write_file(&file_path, &mut package_file, &mut data)
                        .context("Failed to write to package file")?;
                }

                // Extract all the contents of the package.
                for (file_name, file_merkle) in package.contents.iter() {
                    let merkle_string = format!("{}", file_merkle);
                    let mut blob = artifact_reader
                        .open(Path::new(&merkle_string))
                        .context("Failed to read package from blobfs archive(s)")?;

                    let file_path = output.join(file_name);
                    if let Some(parent_dir) = file_path.as_path().parent() {
                        create_dir_all(parent_dir)
                            .context("Failed to create directory for package contents file")?;
                    }
                    let mut packaged_file = create_file(&file_path)
                        .context("Failed to create package contents file")?;
                    std::io::copy(&mut blob, &mut packaged_file).map_err(|err| {
                        anyhow!("Failed to write file at {:?}: {}", file_path, err)
                    })?;
                }
                return Ok(json!({"status": "ok"}));
            }
        }
        Err(anyhow!("Unable to find package with url {}", url))
    }
}
