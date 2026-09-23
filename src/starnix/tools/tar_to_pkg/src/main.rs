// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// tar_to_pkg converts an container TAR image into a format that can be included in a Fuchsia
// package.  The ext4 metadata is stored in a file named "metadata.v1" whilst all the files
// (not directories or symbolic links) are stored in files named with their inode numbers.

use anyhow::Error;
use argh::FromArgs;
use std::fs::File;
use std::path::Path;
use tar_img_extract::docker_archive::DockerArchive;
use tar_img_extract::{InputFormat, tar_img_extract_docker_archive, tar_img_extract_tarball};

fn tar_to_pkg(
    image_path: &str,
    out_dir_path: &Path,
    path_prefix: &str,
    format: InputFormat,
    dep_file: Option<&str>,
) -> Result<(), Error> {
    let input_file = File::open(image_path)?;
    let manifest = match format {
        InputFormat::Tarball => tar_img_extract_tarball(&input_file, &out_dir_path)?,
        InputFormat::DockerArchive => {
            // Load it as a Docker archive.
            let input_archive = DockerArchive::open(input_file)?;
            tar_img_extract_docker_archive(&input_archive, &out_dir_path)?
        }
    };

    // Write a fini manifest
    #[allow(clippy::format_collect, reason = "mass allow for https://fxbug.dev/381896734")]
    std::fs::write(
        format!("{}/manifest.fini", out_dir_path.as_os_str().to_str().unwrap()),
        manifest
            .iter()
            .map(|(dest_path, src_path)| format!("{path_prefix}/{dest_path}={src_path}\n"))
            .collect::<String>(),
    )?;

    if let Some(dep_file) = dep_file {
        #[allow(clippy::format_collect, reason = "mass allow for https://fxbug.dev/381896734")]
        let mut deps: String = manifest.iter().map(|(_, path)| format!("{path} ")).collect();
        deps += &format!(": {image_path}");
        std::fs::write(dep_file, &deps)?;
    }

    Ok(())
}

#[derive(FromArgs)]
/// tar_to_pkg
struct Args {
    /// path to image.
    #[argh(positional)]
    image: String,
    /// output directory.
    #[argh(positional)]
    out_dir: String,
    /// path prefix used for the paths in the manifest.
    #[argh(positional)]
    path_prefix: String,
    /// input format; available formats: "tarball", "docker-archive"
    #[argh(positional)]
    input_format: InputFormat,
    /// a path to a dependency file that should be written.
    #[argh(option, short = 'd')]
    dep_file: Option<String>,
}

fn main() -> Result<(), Error> {
    let args: Args = argh::from_env();
    let _ = std::fs::create_dir(&args.out_dir);
    let out_path = Path::new(&args.out_dir);
    tar_to_pkg(
        &args.image,
        &out_path,
        &args.path_prefix,
        args.input_format,
        args.dep_file.as_deref(),
    )
}
