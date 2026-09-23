// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Clone, Debug, Default)]
pub enum MlockPinFlavor {
    #[default]
    Noop,
    ShadowProcess,
    VmarAlwaysNeed,
}

impl MlockPinFlavor {
    pub fn parse(s: &str) -> Result<Self, anyhow::Error> {
        Ok(match s {
            "noop" => Self::Noop,
            "shadow_process" => Self::ShadowProcess,
            "vmar_always_need" => Self::VmarAlwaysNeed,
            _ => anyhow::bail!(
                "unknown mlock_flavor {s}, known flavors: noop, shadow_process, vmar_always_need"
            ),
        })
    }
}
