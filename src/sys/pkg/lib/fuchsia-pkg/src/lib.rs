// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_use]
pub mod test;

mod build;
mod errors;
mod meta_contents;
mod meta_package;
mod meta_subpackages;
mod package_archive;
mod package_build_manifest;
mod package_builder;
pub mod package_directory;
mod package_manifest;
mod package_manifest_list;
pub mod package_sets;
mod path;
mod path_to_string;
mod subpackages_build_manifest;

pub use crate::errors::{
    BuildError, MetaContentsError, MetaPackageError, MetaSubpackagesError,
    PackageBuildManifestError, PackageManifestError, ParsePackagePathError,
};
pub use crate::meta_contents::MetaContents;
pub use crate::meta_package::MetaPackage;
pub use crate::meta_subpackages::MetaSubpackages;
pub use crate::package_archive::PackageArchiveBuilder;
pub use crate::package_build_manifest::PackageBuildManifest;
pub use crate::package_builder::{ABI_REVISION_FILE_PATH, PackageBuilder};
pub use crate::package_directory::{
    LoadAbiRevisionError, LoadMetaContentsError, PackageDirectory, ReadHashError,
};
pub use crate::package_manifest::{
    BlobInfo, PackageManifest, PackageManifestBuilder, RelativeTo, SubpackageInfo,
};
pub use crate::package_manifest_list::PackageManifestList;
pub use crate::path::{PackageName, PackagePath, PackageVariant};
pub use crate::subpackages_build_manifest::{
    SubpackagesBuildManifest, SubpackagesBuildManifestEntry, SubpackagesBuildManifestEntryKind,
};
pub use fuchsia_url::errors::PackagePathSegmentError;
pub use path_to_string::PathToStringExt;

pub(crate) use crate::build::{BlobEntry, SubpackageEntry};
