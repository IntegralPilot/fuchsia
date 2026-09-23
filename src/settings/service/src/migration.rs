// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The `migration` module exposes structs and traits that can be used to write data migrations for
//! the settings service.
//!
//! # Example:
//! ```
//! let mut builder = MigrationManagerBuilder::new();
//! builder.register(1649199438, Box::new(|file_proxy_generator| async move {
//!     let file_proxy = (file_proxy_generator)("setting_name").await?;
//! }))?;
//! builder.set_dir_proxy(dir_proxy);
//! let migration_manager = builder.build();
//! migration_manager.run_migrations().await?;
//! ```

use anyhow::{Context, Error, anyhow, bail};
use async_trait::async_trait;
use fidl_fuchsia_io::{DirectoryProxy, FileProxy, UnlinkOptions};
use fuchsia_fs::Flags;
use fuchsia_fs::directory::{DirEntry, DirentKind, readdir};
use fuchsia_fs::file::WriteError;
use fuchsia_fs::node::{OpenError, RenameError};

use std::collections::{BTreeMap, HashSet};

/// Errors that can occur during an individual migration.
#[derive(thiserror::Error, Debug)]
pub(crate) enum MigrationError {
    #[error("The data to be migrated from does not exist")]
    NoData,
    #[error("The disk is full so the migration cannot be performed")]
    DiskFull,
    #[error("An unrecoverable error occurred")]
    Unrecoverable(#[source] Error),
}

impl From<Error> for MigrationError {
    fn from(e: Error) -> Self {
        MigrationError::Unrecoverable(e)
    }
}

/// The migration index is just the migration id.
type MigrationIndex = u64;

pub(crate) struct MigrationManager {
    migrations: BTreeMap<MigrationIndex, Box<dyn Migration>>,
    dir_proxy: DirectoryProxy,
}

pub(crate) struct MigrationManagerBuilder {
    migrations: BTreeMap<MigrationIndex, Box<dyn Migration>>,
    id_set: HashSet<u64>,
    dir_proxy: Option<DirectoryProxy>,
}

pub(crate) const MIGRATION_FILE_NAME: &str = "migrations.txt";
const TMP_MIGRATION_FILE_NAME: &str = "tmp_migrations.txt";

impl MigrationManagerBuilder {
    /// Construct a new [MigrationManagerBuilder]
    pub(crate) fn new() -> Self {
        Self { migrations: BTreeMap::new(), id_set: HashSet::new(), dir_proxy: None }
    }

    /// Register a `migration` with a unique `id`. This will fail if another migration with the same
    /// `id` is already registered.
    pub(crate) fn register(&mut self, migration: impl Migration + 'static) -> Result<(), Error> {
        self.register_internal(Box::new(migration))
    }

    fn register_internal(&mut self, migration: Box<dyn Migration>) -> Result<(), Error> {
        let id = migration.id();
        if !self.id_set.insert(id) {
            bail!("migration with id {id} already registered");
        }

        let _ = self.migrations.insert(id, migration);
        Ok(())
    }

    /// Set the directory where migration data will be tracked.
    pub(crate) fn set_migration_dir(&mut self, dir_proxy: DirectoryProxy) {
        self.dir_proxy = Some(dir_proxy);
    }

    /// Construct a [MigrationManager] from the registered migrations and directory proxy. This
    /// method will panic if no directory proxy is registered.
    pub(crate) fn build(self) -> MigrationManager {
        MigrationManager {
            migrations: self.migrations,
            dir_proxy: self.dir_proxy.expect("dir proxy must be provided"),
        }
    }
}

impl MigrationManager {
    /// Run all registered migrations. On success will return final migration number if there are
    /// any registered migrations. On error it will return the most recent migration number that was
    /// able to complete, along with the migration error.
    pub(crate) async fn run_migrations(
        mut self,
    ) -> Result<Option<LastMigration>, (Option<LastMigration>, MigrationError)> {
        let last_migration = {
            let migration_file = fuchsia_fs::directory::open_file(
                &self.dir_proxy,
                MIGRATION_FILE_NAME,
                fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
            )
            .await;
            match migration_file {
                Err(e) => {
                    if let OpenError::OpenError(zx::Status::NOT_FOUND) = e {
                        if let Some(last_migration) = self
                            .initialize_migrations()
                            .await
                            .context("failed to initialize migrations")
                            .map_err(|e| (None, e.into()))?
                        {
                            last_migration
                        } else {
                            // There are no migrations to run.
                            return Ok(None);
                        }
                    } else {
                        return Err((
                            None,
                            Error::from(e).context("failed to load migrations").into(),
                        ));
                    }
                }
                Ok(migration_file) => {
                    let migration_data = fuchsia_fs::file::read_to_string(&migration_file)
                        .await
                        .context("failed to read migrations.txt")
                        .map_err(|e| (None, e.into()))?;
                    let migration_id = migration_data
                        .parse()
                        .context("Failed to parse migration id from migrations.txt")
                        .map_err(|e| (None, e.into()))?;
                    if self.migrations.keys().any(|id| *id == migration_id) {
                        LastMigration { migration_id }
                    } else {
                        log::warn!("Unknown migration {migration_id}, reverting to default data");

                        // We don't know which migrations to run. Some future build may have
                        // run migrations, then something caused a boot loop, and the system
                        // reverted back to this build. This build doesn't know about the
                        // future migrations, so it doesn't know what state the data is in.
                        // Report an error but use the last known migration this build is aware of.
                        // TODO(https://fxbug.dev/42068029) Remove light migration fallback once proper
                        // fallback handling is in place. This is safe for now since Light settings
                        // are always overwritten. The follow-up must be compatible with this
                        // fallback.
                        let last_migration = LastMigration { migration_id: 1653667210 };
                        Self::write_migration_file(&self.dir_proxy, last_migration.migration_id)
                            .await
                            .map_err(|e| (Some(last_migration), e))?;

                        return Err((
                            Some(last_migration),
                            MigrationError::Unrecoverable(anyhow!(format!(
                                "Unknown migration {migration_id}. Using migration with id {}",
                                last_migration.migration_id
                            ))),
                        ));
                    }
                }
            }
        };

        // Track the last migration so we know whether to update the migration file.
        let mut new_last_migration = last_migration;

        // Skip over migrations already previously completed.
        for (id, migration) in self
            .migrations
            .into_iter()
            .skip_while(|&(id, _)| id != last_migration.migration_id)
            .skip(1)
        {
            Self::run_single_migration(
                &self.dir_proxy,
                new_last_migration.migration_id,
                &*migration,
            )
            .await
            .map_err(|e| {
                let e = match e {
                    MigrationError::NoData => MigrationError::NoData,
                    MigrationError::Unrecoverable(e) => {
                        MigrationError::Unrecoverable(e.context(format!("Migration {id} failed")))
                    }
                    _ => e,
                };
                (Some(new_last_migration), e)
            })?;
            new_last_migration = LastMigration { migration_id: id };
        }

        // Remove old files that don't match the current pattern.
        let file_suffix = format!("_{}.pfidl", new_last_migration.migration_id);
        let entries: Vec<DirEntry> = readdir(&self.dir_proxy)
            .await
            .context("another error")
            .map_err(|e| (Some(new_last_migration), e.into()))?;
        let files = entries.into_iter().filter_map(|entry| {
            if let DirentKind::File = entry.kind {
                if entry.name == MIGRATION_FILE_NAME || entry.name.ends_with(&file_suffix) {
                    None
                } else {
                    Some(entry.name)
                }
            } else {
                None
            }
        });

        for file in files {
            self.dir_proxy
                .unlink(&file, &UnlinkOptions::default())
                .await
                .context("failed to remove old file from file system")
                .map_err(|e| (Some(new_last_migration), e.into()))?
                .map_err(zx::Status::err_from_raw)
                .context("another error")
                .map_err(|e| (Some(new_last_migration), e.into()))?;
        }

        Ok(Some(new_last_migration))
    }

    async fn write_migration_file(
        dir_proxy: &DirectoryProxy,
        migration_id: u64,
    ) -> Result<(), MigrationError> {
        // Scope is important. tmp_migration_file needs to be out of scope when the file is
        // renamed.
        {
            let tmp_migration_file = fuchsia_fs::directory::open_file(
                dir_proxy,
                TMP_MIGRATION_FILE_NAME,
                Flags::FLAG_MAYBE_CREATE | fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
            )
            .await
            .context("unable to create migrations file")?;
            if let Err(e) =
                fuchsia_fs::file::write(&tmp_migration_file, migration_id.to_string()).await
            {
                return Err(match e {
                    WriteError::WriteError(zx::Status::NO_SPACE) => MigrationError::DiskFull,
                    _ => Error::from(e).context("failed to write tmp migration").into(),
                });
            };
            if let Err(e) = tmp_migration_file
                .close()
                .await
                .map_err(Error::from)
                .context("failed to close")?
                .map_err(zx::Status::err_from_raw)
            {
                return Err(match e {
                    zx::Status::NO_SPACE => MigrationError::DiskFull,
                    _ => anyhow!("{e:?}").context("failed to properly close migration file").into(),
                });
            }
        }

        if let Err(e) =
            fuchsia_fs::directory::rename(dir_proxy, TMP_MIGRATION_FILE_NAME, MIGRATION_FILE_NAME)
                .await
        {
            return Err(match e {
                RenameError::RenameError(zx::Status::NO_SPACE) => MigrationError::DiskFull,
                _ => Error::from(e).context("failed to rename tmp to migrations.txt").into(),
            });
        };

        if let Err(e) = dir_proxy
            .sync()
            .await
            .map_err(Error::from)
            .context("failed to sync dir")?
            .map_err(zx::Status::err_from_raw)
        {
            match e {
                // This is only returned when the directory is backed by a VFS, so this is fine to
                // ignore.
                zx::Status::NOT_SUPPORTED => {}
                zx::Status::NO_SPACE => return Err(MigrationError::DiskFull),
                _ => {
                    return Err(anyhow!("{e:?}")
                        .context("failed to sync directory for migration id")
                        .into());
                }
            }
        }

        Ok(())
    }

    async fn run_single_migration(
        dir_proxy: &DirectoryProxy,
        old_id: u64,
        migration: &dyn Migration,
    ) -> Result<(), MigrationError> {
        let new_id = migration.id();
        let file_generator = FileGenerator::new(old_id, new_id, Clone::clone(dir_proxy));
        migration.migrate(file_generator).await?;
        Self::write_migration_file(dir_proxy, new_id).await
    }

    /// Runs the initial migration. If it fails due to a [MigrationError::NoData] error, then it
    /// will return the last migration to initialize all of the data sources.
    async fn initialize_migrations(&mut self) -> Result<Option<LastMigration>, MigrationError> {
        // Try to run the first migration. If it fails because there's no data in stash then we can
        // use default values and skip to the final migration number. If it fails because the disk
        // is full, propagate the error up so the main client can decide how to gracefully fallback.
        let &id = if let Some(key) = self.migrations.keys().next() {
            key
        } else {
            return Ok(None);
        };
        let migration = self.migrations.get(&id).unwrap();
        if let Err(migration_error) =
            Self::run_single_migration(&self.dir_proxy, 0, &**migration).await
        {
            match migration_error {
                MigrationError::NoData => {
                    // There was no previous data. We just need to use the default value for all
                    // data and use the most recent migration as the migration number.
                    let migration_id = self
                        .migrations
                        .keys()
                        .last()
                        .copied()
                        .map(|id| LastMigration { migration_id: id });
                    if let Some(last_migration) = migration_id.as_ref() {
                        Self::write_migration_file(&self.dir_proxy, last_migration.migration_id)
                            .await?;
                    }
                    return Ok(migration_id);
                }
                MigrationError::DiskFull => return Err(MigrationError::DiskFull),
                MigrationError::Unrecoverable(e) => {
                    return Err(MigrationError::Unrecoverable(
                        e.context("Failed to run initial migration"),
                    ));
                }
            }
        }

        Ok(Some(LastMigration { migration_id: id }))
    }
}

pub(crate) struct FileGenerator {
    // This will be used when migrating in between pfidl storage types,
    // currently we only have migrations from stash to pfidl so it needs to
    // annotated.
    #[allow(dead_code)]
    old_id: u64,
    new_id: u64,
    dir_proxy: DirectoryProxy,
}

impl FileGenerator {
    pub(crate) fn new(old_id: u64, new_id: u64, dir_proxy: DirectoryProxy) -> Self {
        Self { old_id, new_id, dir_proxy }
    }

    // This will be used when migrating in between pfidl storage types,
    // currently we only have migrations from stash to pfidl so it needs to
    // annotated.
    #[allow(dead_code)]
    pub(crate) async fn old_file(
        &self,
        file_name: impl AsRef<str>,
    ) -> Result<FileProxy, OpenError> {
        let file_name = file_name.as_ref();
        let id = self.new_id;
        fuchsia_fs::directory::open_file(
            &self.dir_proxy,
            &format!("{file_name}_{id}.pfidl"),
            fuchsia_fs::PERM_READABLE,
        )
        .await
    }

    pub(crate) async fn new_file(
        &self,
        file_name: impl AsRef<str>,
    ) -> Result<FileProxy, OpenError> {
        let file_name = file_name.as_ref();
        let id = self.new_id;
        fuchsia_fs::directory::open_file(
            &self.dir_proxy,
            &format!("{file_name}_{id}.pfidl"),
            Flags::FLAG_MAYBE_CREATE | fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
        )
        .await
    }
}

#[async_trait(?Send)]
pub(crate) trait Migration {
    fn id(&self) -> u64;
    async fn migrate(&self, file_generator: FileGenerator) -> Result<(), MigrationError>;
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct LastMigration {
    pub(crate) migration_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_io::DirectoryMarker;
    use futures::FutureExt;
    use futures::future::LocalBoxFuture;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[async_trait(?Send)]
    impl<T> Migration for (u64, T)
    where
        T: Fn(FileGenerator) -> LocalBoxFuture<'static, Result<(), MigrationError>>,
    {
        fn id(&self) -> u64 {
            self.0
        }

        async fn migrate(&self, file_generator: FileGenerator) -> Result<(), MigrationError> {
            (self.1)(file_generator).await
        }
    }

    const ID: u64 = 20_220_130_120_000;
    const ID2: u64 = 20_220_523_120_000;
    const DATA_FILE_NAME: &str = "test_20220130120000.pfidl";

    #[fuchsia::test]
    fn cannot_register_same_id_twice() {
        let mut builder = MigrationManagerBuilder::new();
        builder
            .register((ID, Box::new(|_| async move { Ok(()) }.boxed_local())))
            .expect("should register once");
        let result = builder
            .register((ID, Box::new(|_| async move { Ok(()) }.boxed_local())))
            .map_err(|e| format!("{e:}"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "migration with id 20220130120000 already registered");
    }

    #[fuchsia::test]
    #[should_panic(expected = "dir proxy must be provided")]
    fn cannot_build_migration_manager_without_dir_proxy() {
        let builder = MigrationManagerBuilder::new();
        let _ = builder.build();
    }

    // Needs to be async to create the proxy and stream.
    #[fuchsia::test(allow_stalls = false)]
    async fn can_build_migration_manager_without_migrations() {
        let (proxy, _) = fidl::endpoints::create_proxy_and_stream::<DirectoryMarker>();
        let mut builder = MigrationManagerBuilder::new();
        builder.set_migration_dir(proxy);
        let _migration_manager = builder.build();
    }

    fn open_tempdir(tempdir: &tempfile::TempDir) -> fio::DirectoryProxy {
        fuchsia_fs::directory::open_in_namespace(
            tempdir.path().to_str().expect("tempdir path is not valid UTF-8"),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .expect("failed to open connection to tempdir")
    }

    // Test for initial migration
    #[fuchsia::test]
    async fn if_no_migration_file_runs_initial_migration() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let migration_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let migration_ran = Rc::clone(&migration_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(Clone::clone(&directory));
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(
            result,
            Ok(Some(LastMigration { migration_id: id })) if id == ID
        );
        let migration_file = fuchsia_fs::directory::open_file(
            &directory,
            MIGRATION_FILE_NAME,
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .expect("migration file should exist");
        let migration_number = fuchsia_fs::file::read_to_string(&migration_file)
            .await
            .expect("should be able to read file");
        let migration_number: u64 = dbg!(migration_number).parse().expect("should be a number");
        assert_eq!(migration_number, ID);
    }

    // Test for initial migration
    #[fuchsia::test]
    async fn if_no_migration_file_and_no_data_uses_defaults() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let migration_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let migration_ran = Rc::clone(&migration_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Err(MigrationError::NoData)
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(Clone::clone(&directory));
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(
            result,
            Ok(Some(LastMigration { migration_id: id })) if id == ID
        );
        let migration_file = fuchsia_fs::directory::open_file(
            &directory,
            MIGRATION_FILE_NAME,
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .expect("migration file should exist");
        let migration_number = fuchsia_fs::file::read_to_string(&migration_file)
            .await
            .expect("should be able to read file");
        let migration_number: u64 = dbg!(migration_number).parse().expect("should be a number");
        assert_eq!(migration_number, ID);
    }

    #[fuchsia::test]
    async fn if_no_migration_file_and_no_migrations_no_update() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        builder.set_migration_dir(Clone::clone(&directory));
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(result, Ok(None));
        let open_result = fuchsia_fs::directory::open_file(
            &directory,
            MIGRATION_FILE_NAME,
            fuchsia_fs::PERM_READABLE,
        )
        .await;
        assert_matches!(
            open_result,
            Err(OpenError::OpenError(status)) if status == zx::Status::NOT_FOUND
        );
    }

    #[fuchsia::test]
    async fn if_no_migration_file_but_data_runs_migrations() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let migration_1_ran = Rc::new(AtomicBool::new(false));
        let migration_2_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let migration_ran = Rc::clone(&migration_1_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");

        builder
            .register((
                ID2,
                Box::new({
                    let migration_ran = Rc::clone(&migration_2_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(Clone::clone(&directory));
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(
            result,
            Ok(Some(LastMigration { migration_id: id })) if id == ID2
        );
        assert!(migration_1_ran.load(Ordering::SeqCst));
        assert!(migration_2_ran.load(Ordering::SeqCst));
    }

    #[fuchsia::test]
    async fn if_migration_file_and_up_to_date_no_migrations_run() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        std::fs::write(tempdir.path().join(MIGRATION_FILE_NAME), ID.to_string())
            .expect("failed to write migration file");
        std::fs::write(tempdir.path().join(DATA_FILE_NAME), "").expect("failed to write data file");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let migration_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let migration_ran = Rc::clone(&migration_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(directory);
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(
            result,
            Ok(Some(LastMigration { migration_id: id })) if id == ID
        );
        assert!(!migration_ran.load(Ordering::SeqCst));
    }

    #[fuchsia::test]
    async fn migration_file_exists_but_missing_data() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        std::fs::write(tempdir.path().join(MIGRATION_FILE_NAME), ID.to_string())
            .expect("failed to write migration file");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let initial_migration_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let initial_migration_ran = Rc::clone(&initial_migration_ran);
                    move |_| {
                        let initial_migration_ran = Rc::clone(&initial_migration_ran);
                        async move {
                            initial_migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");

        let second_migration_ran = Rc::new(AtomicBool::new(false));
        builder
            .register((
                ID2,
                Box::new({
                    let second_migration_ran = Rc::clone(&second_migration_ran);
                    move |_| {
                        let second_migration_ran = Rc::clone(&second_migration_ran);
                        async move {
                            second_migration_ran.store(true, Ordering::SeqCst);
                            Err(MigrationError::NoData)
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(directory);
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(
            result,
            Err((Some(LastMigration { migration_id: id }), MigrationError::NoData))
                if id == ID
        );
        assert!(!initial_migration_ran.load(Ordering::SeqCst));
        assert!(second_migration_ran.load(Ordering::SeqCst));
    }

    // Tests that a migration newer than the latest one tracked in the migration file can run.
    #[fuchsia::test]
    async fn migration_file_exists_and_newer_migrations_should_update() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        std::fs::write(tempdir.path().join(MIGRATION_FILE_NAME), ID.to_string())
            .expect("failed to write migration file");
        std::fs::write(tempdir.path().join(DATA_FILE_NAME), "").expect("failed to write data file");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let migration_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let migration_ran = Rc::clone(&migration_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");

        const NEW_CONTENTS: &[u8] = b"test2";
        builder
            .register((
                ID2,
                Box::new({
                    let migration_ran = Rc::clone(&migration_ran);
                    move |file_generator: FileGenerator| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            let file = file_generator.new_file("test").await.expect("can get file");
                            fuchsia_fs::file::write(&file, NEW_CONTENTS)
                                .await
                                .expect("can wite file");
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(Clone::clone(&directory));
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(
            result,
            Ok(Some(LastMigration { migration_id: id })) if id == ID2
        );

        let migration_file = fuchsia_fs::directory::open_file(
            &directory,
            MIGRATION_FILE_NAME,
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .expect("migration file should exist");
        let migration_number: u64 = fuchsia_fs::file::read_to_string(&migration_file)
            .await
            .expect("should be able to read file")
            .parse()
            .expect("should be a number");
        assert_eq!(migration_number, ID2);

        let data_file = fuchsia_fs::directory::open_file(
            &directory,
            &format!("test_{ID2}.pfidl"),
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .expect("migration file should exist");
        let data = fuchsia_fs::file::read(&data_file).await.expect("should be able to read file");
        assert_eq!(data, NEW_CONTENTS);
    }

    #[fuchsia::test]
    async fn migration_file_unknown_id_uses_light_migration() {
        const LIGHT_MIGRATION: u64 = 1653667210;
        const UNKNOWN_ID: u64 = u64::MAX;
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        std::fs::write(tempdir.path().join(MIGRATION_FILE_NAME), UNKNOWN_ID.to_string())
            .expect("failed to write migration file");
        std::fs::write(tempdir.path().join(DATA_FILE_NAME), "").expect("failed to write data file");
        let directory = open_tempdir(&tempdir);
        let mut builder = MigrationManagerBuilder::new();
        let migration_ran = Rc::new(AtomicBool::new(false));

        builder
            .register((
                ID,
                Box::new({
                    let migration_ran = Rc::clone(&migration_ran);
                    move |_| {
                        let migration_ran = Rc::clone(&migration_ran);
                        async move {
                            migration_ran.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        .boxed_local()
                    }
                }),
            ))
            .expect("can register");
        builder.set_migration_dir(Clone::clone(&directory));
        let migration_manager = builder.build();

        let result = migration_manager.run_migrations().await;
        assert_matches!(result, Err((Some(LastMigration {
                    migration_id
                }), MigrationError::Unrecoverable(_)))
            if migration_id == LIGHT_MIGRATION);

        let migration_file = fuchsia_fs::directory::open_file(
            &directory,
            MIGRATION_FILE_NAME,
            fuchsia_fs::PERM_READABLE,
        )
        .await
        .expect("migration file should exist");
        let migration_number: u64 = fuchsia_fs::file::read_to_string(&migration_file)
            .await
            .expect("should be able to read file")
            .parse()
            .expect("should be a number");
        assert_eq!(migration_number, LIGHT_MIGRATION);
    }
}
