// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use either::Either;
use fs_management::filesystem::{
    ServingMultiVolumeFilesystem, ServingSingleVolumeFilesystem, ServingVolume,
};
use storage_stress_test_utils::fvm::FvmInstance;
use stress_test::actor::{Actor, ActorError};

use storage_stress_test_utils::fvm::FvmVolume;

/// An actor that kills the fs instance and destroys the ramdisk
pub struct InstanceActor {
    pub instance: Option<(
        FvmInstance,
        FvmVolume,
        Either<ServingSingleVolumeFilesystem, (ServingMultiVolumeFilesystem, ServingVolume)>,
    )>,
}

impl InstanceActor {
    pub fn new(
        fvm: FvmInstance,
        volume: FvmVolume,
        fs: Either<ServingSingleVolumeFilesystem, (ServingMultiVolumeFilesystem, ServingVolume)>,
    ) -> Self {
        Self { instance: Some((fvm, volume, fs)) }
    }
}

#[async_trait]
impl Actor for InstanceActor {
    async fn perform(&mut self) -> Result<(), ActorError> {
        if let Some((fvm_instance, _volume, fs)) = self.instance.take() {
            // We want to kill the ram-disk before the filesystem so that we test the filesystem in
            // a simulated power-fail.
            // Wait for the device to go away.
            std::mem::drop(fvm_instance);

            // Ignore errors: we've yanked the underlying device, and filesystems have different
            // ways of handling that, and it doesn't really matter how they handle it i.e. crashing
            // is fine.
            match fs {
                Either::Left(fs) => {
                    let _ = fs.kill().await;
                }
                // TODO(https://fxbug.dev/42057166): Make termination more abrupt.
                Either::Right((fs, _)) => {
                    let _ = fs.shutdown().await;
                }
            }
        } else {
            panic!("Instance was already killed!")
        }
        Err(ActorError::ResetEnvironment)
    }
}
