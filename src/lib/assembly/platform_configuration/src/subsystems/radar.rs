// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use assembly_constants::BoardFeature;

pub(crate) struct RadarSubsystemConfig;
impl DefineSubsystemConfiguration<()> for RadarSubsystemConfig {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        _: &(),
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if context.board_config.provides_feature(BoardFeature::Radar)
            && matches!(context.feature_set_level, FeatureSetLevel::Standard)
        {
            if matches!(context.build_type, BuildType::Eng | BuildType::UserDebug) {
                builder.platform_bundle("radar_proxy_with_injector")?;
            } else {
                builder.platform_bundle("radar_proxy_without_injector")?;
            }
        }

        Ok(())
    }
}
