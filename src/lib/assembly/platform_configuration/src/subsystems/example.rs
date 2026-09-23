// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START example_patches]
use crate::subsystems::prelude::*;
use assembly_config_schema::platform_settings::example_config::ExampleConfig;

pub(crate) struct ExampleSubsystemConfig;
impl DefineSubsystemConfiguration<ExampleConfig> for ExampleSubsystemConfig {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        example_config: &ExampleConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        // If the build-type is eng, enabled "foo" for the example component.
        builder
            .package("configured_by_assembly")
            .component("meta/to_configure.cm")?
            .field("enable_foo", matches!(context.build_type, BuildType::Eng))?;

        if example_config.include_example_aib {
            builder.platform_bundle("example_assembly_bundle")?;

            // Set structured config for a component in the example AIB
            let mut component_config = builder.package("for-test").component("meta/bar.cm")?;
            component_config.field("asserted_by_scrutiny_test", "check this string!")?;
            component_config.field("mutable_by_parent_config", "don't check this string!")?;
        }

        Ok(())
    }
}
// [END example_patches]
