// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use assembly_config_schema::platform_settings::factory_store_providers_config::FactoryStoreProvidersConfig;

pub(crate) struct FactoryStoreProvidersSubsystem;
impl DefineSubsystemConfiguration<FactoryStoreProvidersConfig> for FactoryStoreProvidersSubsystem {
    fn define_configuration(
        _context: &ConfigurationContext<'_>,
        config: &FactoryStoreProvidersConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        builder.package("factory_store_providers").optional_config_data_files(vec![
            (&config.alpha, "fuchsia.factory.AlphaFactoryStoreProvider.config"),
            (
                &config.cast_credentials,
                "fuchsia.factory.CastCredentialsFactoryStoreProvider.config",
            ),
            (&config.misc, "fuchsia.factory.MiscFactoryStoreProvider.config"),
            (&config.play_ready, "fuchsia.factory.PlayReadyFactoryStoreProvider.config"),
            (&config.weave, "fuchsia.factory.WeaveFactoryStoreProvider.config"),
            (&config.widevine, "fuchsia.factory.WidevineFactoryStoreProvider.config"),
        ])?;

        Ok(())
    }
}
