// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use assembly_config_schema::product_settings::BuildInfoConfig;
use assembly_constants::{FileEntry, PackageDestination, PackageSetDestination};

pub(crate) struct BuildInfoSubsystem;
impl DefineSubsystemConfiguration<Option<BuildInfoConfig>> for BuildInfoSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        build_info: &Option<BuildInfoConfig>,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        if let Some(build_info) = &build_info {
            let dir = builder
                .add_domain_config(PackageSetDestination::Blob(PackageDestination::BuildInfo))
                .skip_expose()
                .directory("data");
            dir.entry_from_contents("board", &context.board_config.name)?;
            dir.entry_from_contents("product", &build_info.name)?;
            dir.entry(FileEntry {
                source: build_info.version.clone(),
                destination: "version".into(),
            })?;
            dir.entry(FileEntry {
                source: build_info.version.clone(),
                destination: "product_version".into(),
            })?;
            dir.entry(FileEntry {
                source: context.get_resource("version.txt"),
                destination: "platform_version".into(),
            })?;
            dir.entry(FileEntry {
                source: build_info.jiri_snapshot.clone(),
                destination: "snapshot".into(),
            })?;
            dir.entry(FileEntry {
                source: build_info.latest_commit_date.clone(),
                destination: "latest-commit-date".into(),
            })?;
        }
        Ok(())
    }
}
