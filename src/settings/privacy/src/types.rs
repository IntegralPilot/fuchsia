// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use settings_common::inspect::event::Nameable;

#[derive(PartialEq, Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PrivacyInfo {
    pub user_data_sharing_consent: Option<bool>,
}

impl Nameable for PrivacyInfo {
    const NAME: &str = "Privacy";
}
