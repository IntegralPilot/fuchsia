// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fs_management::filesystem::ServingSingleVolumeFilesystem;
use storage_stress_test_utils::fvm::{FvmInstance, FvmVolume};
use stress_test::actor::{Actor, ActorError};

/// An actor that kills blobfs and destroys the ramdisk
pub struct InstanceActor {
    pub instance: Option<(ServingSingleVolumeFilesystem, FvmInstance, FvmVolume)>,
}

impl InstanceActor {
    pub fn new(fvm: FvmInstance, blobfs: ServingSingleVolumeFilesystem, volume: FvmVolume) -> Self {
        Self { instance: Some((blobfs, fvm, volume)) }
    }
}

#[async_trait]
impl Actor for InstanceActor {
    async fn perform(&mut self) -> Result<(), ActorError> {
        if let Some((blobfs, _, _)) = self.instance.take() {
            // TODO(https://fxbug.dev/42057166): Make termination more abrupt.
            blobfs.shutdown().await.expect("Could not kill blobfs");
        } else {
            panic!("Instance was already killed!")
        }
        Err(ActorError::ResetEnvironment)
    }
}
