// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! A Manager for downloading and storing assembly artifacts.

mod artifact;
mod artifact_cache;
mod build_api;
mod cipd;
mod gn_label;
pub mod mos;

pub use artifact::{Artifact, ArtifactType, CIPDPackage, MOSIdentifier, Slot};
pub use artifact_cache::{ArtifactCache, ArtifactError};
pub use mos::MOSClient;
