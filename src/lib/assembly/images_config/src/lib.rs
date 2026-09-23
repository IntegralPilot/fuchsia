// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for reading and writing a config describing which images to
//! generate and how.

mod board_filesystem_config;
mod images_config;
mod product_filesystem_config;

pub use images_config::{
    BlobFS, EmptyData, Fvm, FvmFilesystem, FvmOutput, Fxfs, Image, ImagesConfig, NandFvm, Reserved,
    SparseFvm, StandardFvm, VBMeta, Zbi,
};

pub use board_filesystem_config::{
    BoardFilesystemConfig, BuildSpecificSize, FlexibleSize, FxfsBlobFormat, GptMode,
    PostProcessingScript, VBMetaDescriptor, VBMetaStyle, ZbiCompression,
};

pub use product_filesystem_config::{
    BlobFvmVolumeConfig, BlobfsLayout, DataFilesystemFormat, DataFvmVolumeConfig,
    FilesystemImageMode, FvmConfig, ImageName, ProductFilesystemConfig, VolumeConfig,
};

/// Returns true if the value is equal to its default.
fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    t == &T::default()
}
