// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod docker_archive;
pub mod layered_image;
pub mod populate_bundle_visitor;

use crate::docker_archive::DockerArchive;
use crate::layered_image::LayeredImage;
use crate::populate_bundle_visitor::PopulateBundleVisitor;
use anyhow::{Error, bail};
use ext4_metadata::{ROOT_INODE_NUM, S_IFDIR};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::str::FromStr;

pub enum InputFormat {
    /// A tarball containing the root filesystem.
    Tarball,

    /// A Docker archive (created with "docker save").
    DockerArchive,
}

impl FromStr for InputFormat {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "tarball" => Ok(InputFormat::Tarball),
            "docker-archive" => Ok(InputFormat::DockerArchive),
            other => bail!("Invalid input format: {}", other),
        }
    }
}

/// Extract the files from a tarball at `input_file` and return a map of the destination
/// to source pairs. Additionally, create a metadata file that provides information
/// necessary for mounting the files from a fuchsia package.
pub fn tar_img_extract_tarball(
    input_file: &File,
    out_dir: &Path,
) -> Result<HashMap<String, String>, Error> {
    // Create an empty LayeredImage and fill it with files from input archive.
    let input_archive = tar::Archive::new(input_file);
    let mut image = LayeredImage::new(&out_dir)?;
    image = image.add_layer(input_archive, false)?;
    tar_img_extract(image, out_dir)
}

/// Extract the files from a docker archive at `input_archive` and return a map of the destination
/// to source pairs. Additionally, create a metadata file that provides information
/// necessary for mounting the files from a fuchsia package.
pub fn tar_img_extract_docker_archive(
    input_archive: &DockerArchive,
    out_dir: &Path,
) -> Result<HashMap<String, String>, Error> {
    // Create an empty LayeredImage and fill it with files from input archive.
    let mut image = LayeredImage::new(&out_dir)?;
    // Load each layer.
    for (i, layer) in input_archive.layers()?.enumerate() {
        let handle_whiteouts = i != 0; // the base layer cannot contain whiteouts
        image = image.add_layer(layer, handle_whiteouts)?;
    }
    tar_img_extract(image, out_dir)
}

/// Return a map of destination to source pairs based on the provided layered image at `image`.
/// Additionally, create a metadata file that provides information
/// necessary for mounting the files from a fuchsia package.
fn tar_img_extract(
    mut image: LayeredImage,
    out_dir: &Path,
) -> Result<HashMap<String, String>, Error> {
    // Ensure that all of the container's mountpoints actually exist.
    image = image.ensure_directory_exists("/dev")?;
    image = image.ensure_directory_exists("/proc")?;
    image = image.ensure_directory_exists("/sys")?;
    image = image.ensure_directory_exists("/tmp")?;

    // Assign inode numbers and get the root directory.
    let root_dir = {
        let mut next_inode_num = ROOT_INODE_NUM;
        let mut inode_num_generator = || {
            let result = next_inode_num;
            next_inode_num += 1;
            result
        };
        image.finalize(&mut inode_num_generator)
    };
    assert_eq!(root_dir.metadata().inode_num(), ROOT_INODE_NUM);

    // Convert the hierarchy we just loaded into an ext4 Metadata object and its blobs.
    let mut bundle_visitor = PopulateBundleVisitor::new();
    bundle_visitor.ext4_metadata.insert_directory(
        ROOT_INODE_NUM,
        root_dir.metadata().mode() | S_IFDIR,
        root_dir.metadata().uid(),
        root_dir.metadata().gid(),
        root_dir.metadata().extended_attributes().clone().into(),
    );
    root_dir.visit(&mut bundle_visitor);

    // Serialize and write the ext4 Metadata object to disk.
    let metadata_v1 = bundle_visitor.ext4_metadata.serialize();
    let metadata_v1_path = format!("{}/metadata.v1", out_dir.as_os_str().to_str().unwrap());
    std::fs::write(&metadata_v1_path, &metadata_v1)?;
    bundle_visitor.manifest.insert("metadata.v1".to_string(), metadata_v1_path);

    Ok(bundle_visitor.manifest)
}
