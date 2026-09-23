// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for constructing and serializing the VBMeta struct for verified boot.

mod descriptor;
mod footer;
mod header;
mod key;
mod test;
mod vbmeta;

pub use descriptor::{Salt, SaltError};

pub use vbmeta::{
    ChainPartition, HashDescriptor, RawHashDescriptor, VBMeta, VBMetaBuilder, VBMetaConfig,
    VBMetaOutput,
};
