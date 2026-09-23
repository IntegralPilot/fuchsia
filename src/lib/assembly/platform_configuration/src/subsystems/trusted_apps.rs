// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, Result};
use assembly_config_schema::product_settings::TrustedApp as ProductTrustedApp;
use assembly_config_schema::product_settings::TrustedAppType::{BinderRPC, GlobalPlatform};
use assembly_constants::{BootfsPackageDestination, PackageSetDestination};
use assembly_images_config::FilesystemImageMode;
use fuchsia_tee_manager_config::TAConfig;

pub(crate) struct TrustedAppsSubsystem;
impl DefineSubsystemConfiguration<(&Vec<ProductTrustedApp>, FilesystemImageMode)>
    for TrustedAppsSubsystem
{
    fn define_configuration(
        _context: &ConfigurationContext<'_>,
        config: &(&Vec<ProductTrustedApp>, FilesystemImageMode),
        builder: &mut dyn ConfigurationBuilder,
    ) -> Result<()> {
        let (trusted_apps, image_mode) = config;
        if trusted_apps.is_empty() {
            return Ok(());
        }

        builder.platform_bundle("trusted_execution_environment")?;

        // Create a domain config package for all the TA configs.
        let dir = builder
            .add_domain_config(PackageSetDestination::Boot(
                BootfsPackageDestination::TaManagerConfig,
            ))
            .directory("config");

        // Add the configs for all the TAs.
        for c in trusted_apps.iter() {
            let url = if *image_mode == FilesystemImageMode::NoImage {
                let url = fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl::parse(&c.component_url)
                    .with_context(|| format!("Parsing: {}", c.component_url))?;
                fuchsia_url::boot::AbsoluteComponentUrl::new(
                    Some(
                        fuchsia_url::Path::try_from(url.path()).with_context(|| {
                            format!("Convert to a boot url path: {}", url.path())
                        })?,
                    ),
                    url.into_package_and_resource().1,
                )
                .to_string()
            } else {
                c.component_url.clone()
            };
            let ta_config = match c.ta_type {
                GlobalPlatform => TAConfig::global_platform(url),
                BinderRPC => TAConfig::binder_rpc(url),
            };
            let ta_config = serde_json::to_string(&ta_config)
                .with_context(|| format!("Failed to serialize the ta config: {}", c.guid))?;
            dir.entry_from_contents(&format!("{}.json", c.guid), &ta_config)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileOrContents;
    use crate::subsystems::ConfigurationBuilderImpl;

    #[test]
    fn test_convert_component_urls_to_boot_urls() {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &Default::default(),
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let trusted_apps = vec![
            ProductTrustedApp {
                component_url: "fuchsia-pkg://fuchsia.com/pkg1#resource1.txt".into(),
                guid: "abcdef".into(),
                ta_type: GlobalPlatform,
            },
            ProductTrustedApp {
                component_url: "fuchsia-pkg://fuchsia.com/pkg2#resource2.txt".into(),
                guid: "123456".into(),
                ta_type: BinderRPC,
            },
        ];
        let image_mode = FilesystemImageMode::NoImage;
        let result = TrustedAppsSubsystem::define_configuration(
            &context,
            &(&trusted_apps, image_mode),
            &mut builder,
        );
        assert!(result.is_ok());
        let config = builder.build();
        let domain_config = config
            .domain_configs
            .get(&PackageSetDestination::Boot(BootfsPackageDestination::TaManagerConfig))
            .unwrap();
        let directory = domain_config.directories.get("config").unwrap();
        let entries: Vec<FileOrContents> = directory.entries.values().cloned().collect();
        let expected_configs = [
            TAConfig::binder_rpc("fuchsia-boot:///pkg2#resource2.txt".into()),
            TAConfig::global_platform("fuchsia-boot:///pkg1#resource1.txt".into()),
        ];
        let expected_configs: Vec<FileOrContents> = expected_configs
            .iter()
            .map(|c| FileOrContents::Contents(serde_json::to_string(c).unwrap()))
            .collect();
        assert_eq!(entries, expected_configs);
    }
}
