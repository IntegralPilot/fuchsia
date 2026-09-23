// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::VdsoError;
use fuchsia_runtime::{HandleInfo, HandleType, take_startup_handle};
use std::collections::HashMap;
use std::sync::LazyLock;

fn take_vdso_vmos() -> Result<HashMap<zx::Name, zx::Vmo>, VdsoError> {
    let mut vmos = HashMap::new();
    let mut i = 0;
    while let Some(handle) = take_startup_handle(HandleInfo::new(HandleType::VdsoVmo, i)) {
        let vmo = zx::Vmo::from(handle);
        let name = vmo.get_name().map_err(VdsoError::GetName)?;
        vmos.insert(name, vmo);
        i += 1;
    }
    Ok(vmos)
}

pub fn get_vdso_vmo(name: &zx::Name) -> Result<zx::Vmo, VdsoError> {
    static VMOS: LazyLock<HashMap<zx::Name, zx::Vmo>> =
        LazyLock::new(|| take_vdso_vmos().expect("Failed to take vDSO VMOs"));
    if let Some(vmo) = VMOS.get(name) {
        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
            .map_err(|status| VdsoError::CouldNotDuplicate { name: *name, status })
    } else {
        Err(VdsoError::NotFound(*name))
    }
}

/// Returns an owned VMO handle to the stable vDSO, duplicated from the handle
/// provided to this process through its processargs bootstrap message.
pub fn get_stable_vdso_vmo() -> Result<zx::Vmo, VdsoError> {
    get_vdso_vmo(&zx::Name::new_lossy("vdso/stable"))
}

/// Returns an owned VMO handle to the next vDSO, duplicated from the handle
/// provided to this process through its processargs bootstrap message.
pub fn get_next_vdso_vmo() -> Result<zx::Vmo, VdsoError> {
    get_vdso_vmo(&zx::Name::new_lossy("vdso/next"))
}
