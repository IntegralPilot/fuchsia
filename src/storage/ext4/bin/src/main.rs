// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ext4_to_pkg converts an ext4 image (embedded within an Android sparse image) into a format that
// can be included in a Fuchsia package.  The ext4 metadata is stored in a file named "metadata.v1"
// whilst all the files (not directories or symbolic links) are stored in files named with their
// inode numbers.

use anyhow::Error;
use argh::FromArgs;
use ext4_extract::ext4_extract;

fn ext4_to_pkg(
    path: &str,
    out_dir: &str,
    path_prefix: &str,
    dep_file: Option<&str>,
) -> Result<(), Error> {
    let manifest = ext4_extract(path, out_dir)?;

    // Write a fini manifest
    #[allow(clippy::format_collect, reason = "mass allow for https://fxbug.dev/381896734")]
    std::fs::write(
        format!("{out_dir}/manifest.fini"),
        manifest
            .iter()
            .map(|(inode_num, path)| format!("{path_prefix}/{inode_num}={path}\n"))
            .collect::<String>(),
    )?;

    if let Some(dep_file) = dep_file {
        #[allow(clippy::format_collect, reason = "mass allow for https://fxbug.dev/381896734")]
        let mut deps: String = manifest.iter().map(|(_, path)| format!("{path} ")).collect();
        deps += &format!(": {path}");
        std::fs::write(dep_file, &deps)?;
    }

    Ok(())
}

#[derive(FromArgs, PartialEq, Debug)]
/// ext4_to_pkg
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
    /// a path to a dependency file that should be written.
    #[argh(option, short = 'd')]
    dep_file: Option<String>,
}

fn main() -> Result<(), Error> {
    let args: Args = argh::from_env();
    let _ = std::fs::create_dir(&args.out_dir);
    ext4_to_pkg(&args.image, &args.out_dir, &args.path_prefix, args.dep_file.as_deref())
}
