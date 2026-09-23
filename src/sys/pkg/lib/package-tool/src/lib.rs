// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42055130): Consider enabling globally.
#![deny(unused_crate_dependencies)]

use anyhow::{Context as _, Result};
use camino::Utf8Path;
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

mod args;
mod package_archive;
mod package_build;
mod repo_create;
mod repo_publish;

pub use crate::args::{
    PackageArchiveAddCommand, PackageArchiveCreateCommand, PackageArchiveEditCommand,
    PackageArchiveExtractCommand, PackageArchiveRemoveCommand, PackageBuildCommand,
    RepoCreateCommand, RepoPMListCommand, RepoPublishCommand,
};
pub use crate::package_archive::{
    cmd_package_archive_add, cmd_package_archive_create, cmd_package_archive_edit,
    cmd_package_archive_extract, cmd_package_archive_remove,
};
pub use crate::package_build::cmd_package_build;
pub use crate::repo_create::cmd_repo_create;
pub use crate::repo_publish::{cmd_repo_package_manifest_list, cmd_repo_publish};

pub(crate) const PACKAGE_MANIFEST_NAME: &str = "package_manifest.json";
pub(crate) const BLOBS_JSON_NAME: &str = "blobs.json";

pub(crate) fn to_writer_json_pretty(
    writer: impl Write,
    value: impl Serialize,
) -> serde_json::Result<()> {
    let mut ser = serde_json::ser::Serializer::with_formatter(
        BufWriter::new(writer),
        serde_json::ser::PrettyFormatter::with_indent(b"    "),
    );

    value.serialize(&mut ser)
}

/// Spaces are separators, so spaces in filenames must be escaped.
pub(crate) fn convert_to_depfile_filepath(path: &str) -> String {
    path.replace(' ', "\\ ")
}

/// Writing a depfile at location `path` in format described at:
/// https://fuchsia.dev/fuchsia-src/development/build/hermetic_actions#depfiles
///
/// `dst`: `srcs_0` `srcs_1` ...
pub(crate) fn write_depfile(
    path: &Path,
    dst: &Utf8Path,
    srcs: impl Iterator<Item = String>,
) -> Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut file_writer = BufWriter::new(file);

    let dep_str = format!(
        "{}: {}",
        convert_to_depfile_filepath(dst.as_str()),
        srcs.map(|x| convert_to_depfile_filepath(x.as_str())).collect::<Vec<_>>().join(" "),
    );

    write!(file_writer, "{dep_str}")?;

    Ok(())
}
