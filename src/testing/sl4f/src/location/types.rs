// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_location_position::{Position, PositionExtras};
use serde::{Serialize, Serializer};

#[derive(Serialize)]
#[serde(remote = "Position")]
struct PositionDef {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(with = "PositionExtrasDef")]
    pub extras: PositionExtras,
}

#[derive(Serialize)]
struct PositionExtrasDef {
    pub accuracy_meters: Option<f64>,
    pub altitude_meters: Option<f64>,
}

impl PositionExtrasDef {
    // We implement this manually instead of using #[serde(remote = "PositionExtras")]
    // to uphold FIDL's guarantee that adding table fields is source compatible.
    fn serialize<S: Serializer>(
        &PositionExtras { accuracy_meters, altitude_meters, .. }: &PositionExtras,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Self { accuracy_meters, altitude_meters }.serialize(serializer)
    }
}

#[derive(Serialize)]
pub struct PositionSerializer(#[serde(with = "PositionDef")] pub Position);
