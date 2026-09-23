// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{ConfigurationBuilder, ConfigurationContext, DefineSubsystemConfiguration};

pub struct RcsSubsystemConfig;
impl DefineSubsystemConfiguration<()> for RcsSubsystemConfig {
    fn define_configuration(
        _context: &ConfigurationContext<'_>,
        _config: &(),
        _builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
