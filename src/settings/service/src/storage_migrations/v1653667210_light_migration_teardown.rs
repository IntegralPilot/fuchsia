// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::migration::{FileGenerator, Migration, MigrationError};
use anyhow::{anyhow, Context};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_stash::StoreProxy;

const LIGHT_KEY: &str = "settings_light_info";

/// Deletes old Light settings data from stash.
pub(crate) struct V1653667210LightMigrationTeardown(pub(crate) StoreProxy);

#[async_trait::async_trait(?Send)]
impl Migration for V1653667210LightMigrationTeardown {
    fn id(&self) -> u64 {
        1653667210
    }

    async fn migrate(&self, _: FileGenerator) -> Result<(), MigrationError> {
        let (stash_proxy, server_end) = create_proxy();
        self.0.create_accessor(false, server_end).expect("failed to create accessor for stash");
        stash_proxy.delete_value(LIGHT_KEY).context("failed to call delete_value")?;
        stash_proxy.commit().context("failed to commit deletion of old light key")?;
        drop(stash_proxy);

        let (stash_proxy, server_end) = create_proxy();
        self.0.create_accessor(true, server_end).expect("failed to create accessor for stash");
        let value = stash_proxy.get_value(LIGHT_KEY).await.context("failed to call get_value")?;
        if value.is_some() {
            Err(MigrationError::Unrecoverable(anyhow!("failed to delete stash data")))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_migrations::tests::open_tempdir;
    use assert_matches::assert_matches;
    use fidl_fuchsia_stash::{StoreAccessorRequest, StoreMarker, StoreRequest, Value};
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Ensure the teardown deletes and commits the deletion of data from stash.
    #[fuchsia::test]
    async fn v1653667208_light_migration_teardown_test() {
        let (store_proxy, server_end) = create_proxy::<StoreMarker>();
        let mut request_stream = server_end.into_stream();
        let commit_called = Rc::new(AtomicBool::new(false));
        let task = fasync::Task::local({
            let commit_called = Rc::clone(&commit_called);
            async move {
                let mut tasks = vec![];
                while let Some(Ok(request)) = request_stream.next().await {
                    if let StoreRequest::CreateAccessor { accessor_request, .. } = request {
                        let mut request_stream = accessor_request.into_stream();
                        tasks.push(fasync::Task::local({
                            let commit_called = Rc::clone(&commit_called);
                            async move {
                                while let Some(Ok(request)) = request_stream.next().await {
                                    match request {
                                        StoreAccessorRequest::DeleteValue { key, .. } => {
                                            assert_eq!(key, LIGHT_KEY);
                                        }
                                        StoreAccessorRequest::Commit { .. } => {
                                            commit_called.store(true, Ordering::SeqCst);
                                        }
                                        StoreAccessorRequest::GetValue { key, responder } => {
                                            assert_eq!(key, LIGHT_KEY);
                                            responder
                                                .send(None)
                                                .expect("should be able to send response");
                                        }
                                        _ => panic!("unexpected request: {request:?}"),
                                    }
                                }
                            }
                        }))
                    }
                }
                for task in tasks {
                    task.await
                }
            }
        });

        let migration = V1653667210LightMigrationTeardown(store_proxy);
        let fs = tempfile::tempdir().expect("failed to create tempdir");
        let directory = open_tempdir(&fs);
        let file_generator = FileGenerator::new(0, migration.id(), Clone::clone(&directory));
        assert_matches!(migration.migrate(file_generator).await, Ok(()));

        drop(migration);

        task.await;
        assert!(commit_called.load(Ordering::SeqCst));
    }

    // Ensure we report an unrecoverable error if we're unable to delete the data from stash.
    #[fuchsia::test]
    async fn v1653667208_light_migration_teardown_commit_fails() {
        let (store_proxy, server_end) = create_proxy::<StoreMarker>();
        let mut request_stream = server_end.into_stream();
        let task = fasync::Task::local(async move {
            let mut tasks = vec![];
            while let Some(Ok(request)) = request_stream.next().await {
                if let StoreRequest::CreateAccessor { accessor_request, .. } = request {
                    let mut request_stream = accessor_request.into_stream();
                    tasks.push(fasync::Task::local(async move {
                        while let Some(Ok(request)) = request_stream.next().await {
                            match request {
                                StoreAccessorRequest::DeleteValue { .. } => {
                                    // no-op
                                }
                                StoreAccessorRequest::Commit { .. } => {
                                    // no-op
                                }
                                StoreAccessorRequest::GetValue { key, responder } => {
                                    assert_eq!(key, LIGHT_KEY);
                                    responder
                                        .send(Some(Value::Stringval("data".to_owned())))
                                        .expect("should be able to send response");
                                }
                                _ => panic!("unexpected request: {request:?}"),
                            }
                        }
                    }))
                }
            }
            for task in tasks {
                task.await
            }
        });

        let migration = V1653667210LightMigrationTeardown(store_proxy);
        let fs = tempfile::tempdir().expect("failed to create tempdir");
        let directory = open_tempdir(&fs);
        let file_generator = FileGenerator::new(0, migration.id(), Clone::clone(&directory));
        let result = migration.migrate(file_generator).await;
        assert_matches!(result, Err(MigrationError::Unrecoverable(_)));
        assert!(format!("{:?}", result.unwrap_err()).contains("failed to delete stash data"));

        drop(migration);

        task.await;
    }
}
