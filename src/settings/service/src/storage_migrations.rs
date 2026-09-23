// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::base::SettingType;
use crate::migration::{MigrationError, MigrationManager, MigrationManagerBuilder};
use fidl_fuchsia_io::DirectoryProxy;
use fidl_fuchsia_stash::StoreProxy;
use std::collections::HashSet;
use v1653667208_light_migration::V1653667208LightMigration;
use v1653667210_light_migration_teardown::V1653667210LightMigrationTeardown;

mod v1653667208_light_migration;
mod v1653667210_light_migration_teardown;

pub(crate) fn register_migrations(
    settings: &HashSet<SettingType>,
    migration_dir: DirectoryProxy,
    store_proxy: StoreProxy,
) -> Result<MigrationManager, MigrationError> {
    let mut builder = MigrationManagerBuilder::new();
    builder.set_migration_dir(migration_dir);
    if settings.contains(&SettingType::Light) {
        builder.register(V1653667208LightMigration(store_proxy.clone()))?;
        builder.register(V1653667210LightMigrationTeardown(store_proxy))?;
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_io::DirectoryMarker;
    use fidl_fuchsia_stash::StoreMarker;

    // Run migration registration with all settings and policies turned on so we can ensure there's
    // no issues with registering any of the migrations.
    #[fuchsia::test(allow_stalls = false)]
    async fn ensure_unique_ids() {
        let mut settings = HashSet::new();
        let _ = settings.insert(SettingType::Accessibility);
        let _ = settings.insert(SettingType::Audio);
        let _ = settings.insert(SettingType::Display);
        let _ = settings.insert(SettingType::DoNotDisturb);
        let _ = settings.insert(SettingType::FactoryReset);
        let _ = settings.insert(SettingType::Input);
        let _ = settings.insert(SettingType::Intl);
        let _ = settings.insert(SettingType::Keyboard);
        let _ = settings.insert(SettingType::Light);
        let _ = settings.insert(SettingType::NightMode);
        let _ = settings.insert(SettingType::Privacy);
        let _ = settings.insert(SettingType::Setup);
        let (directory_proxy, _) = create_proxy::<DirectoryMarker>();
        let (store_proxy, _) = create_proxy::<StoreMarker>();
        if let Err(e) = register_migrations(&settings, directory_proxy, store_proxy) {
            panic!("Unable to register migrations: Err({e:?})");
        }
    }

    // Opens a FIDL connection to a `TempDir`.
    pub(crate) fn open_tempdir(tempdir: &tempfile::TempDir) -> DirectoryProxy {
        fuchsia_fs::directory::open_in_namespace(
            tempdir.path().to_str().expect("tempdir path is not valid UTF-8"),
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
        )
        .expect("failed to open connection to tempdir")
    }
}
