// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::configuration_without_recovery::ConfigurationWithoutRecovery;
use super::errors::{BootManagerError, BootManagerResultExt};
use fidl_fuchsia_paver as paver;

/// Commit the slot by updating the boot metadata.
pub async fn do_commit(
    boot_manager: &paver::BootManagerProxy,
    current_config: &ConfigurationWithoutRecovery,
) -> Result<(), BootManagerError> {
    // Do all writes inside this function to ensure that we call flush no matter what.
    async fn internal_write(
        boot_manager: &paver::BootManagerProxy,
        current_config: &ConfigurationWithoutRecovery,
    ) -> Result<(), BootManagerError> {
        let alternate_config = current_config.to_alternate().into();

        let () = boot_manager
            .set_configuration_healthy(current_config.into())
            .await
            .into_boot_manager_result("set_configuration_healthy")?;
        let () = boot_manager
            .set_configuration_unbootable(alternate_config)
            .await
            .into_boot_manager_result("set_configuration_unbootable")?;
        Ok(())
    }

    // Capture the result of the writes so we can return it after we flush.
    let write_result = internal_write(boot_manager, current_config).await;

    let () = boot_manager.flush().await.into_boot_manager_result("flush")?;

    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use mock_paver::{MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
    use std::sync::Arc;
    use zx::Status;

    /// Helper fn to verify that do_commit succeeds.
    async fn run_success_test(current_config: &ConfigurationWithoutRecovery) {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let () = do_commit(&paver.spawn_boot_manager_service(), current_config).await.unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::SetConfigurationHealthy { configuration: current_config.into() },
                PaverEvent::SetConfigurationUnbootable {
                    configuration: current_config.to_alternate().into()
                },
                PaverEvent::BootManagerFlush,
            ]
        );
    }

    #[fuchsia::test]
    async fn test_success_current_a() {
        run_success_test(&ConfigurationWithoutRecovery::A).await;
    }

    #[fuchsia::test]
    async fn test_success_current_b() {
        run_success_test(&ConfigurationWithoutRecovery::B).await;
    }

    #[fuchsia::test]
    async fn test_fails_when_set_healthy_fails() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::return_error(|e| match e {
                    PaverEvent::SetConfigurationHealthy { .. } => Err(Status::OUT_OF_RANGE),
                    _ => Ok(()),
                }))
                .build(),
        );

        assert_matches!(
            do_commit(&paver.spawn_boot_manager_service(), &ConfigurationWithoutRecovery::A).await,
            Err(BootManagerError::Status {
                method_name: "set_configuration_healthy",
                status: Status::OUT_OF_RANGE
            })
        );

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::SetConfigurationHealthy { configuration: paver::Configuration::A },
                PaverEvent::BootManagerFlush,
            ]
        );
    }
}
