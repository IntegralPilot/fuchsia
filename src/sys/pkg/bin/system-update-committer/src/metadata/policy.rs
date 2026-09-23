// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::configuration_without_recovery::ConfigurationWithoutRecovery;
use super::errors::{BootManagerError, BootManagerResultExt, PolicyError};
use fidl_fuchsia_paver as paver;
use log::{info, warn};
use zx::Status;

/// After gathering state from the BootManager, the PolicyEngine can answer whether we
/// should verify and commit.
#[derive(Debug)]
pub struct PolicyEngine(State);

#[derive(Debug)]
enum State {
    // If no verification or committing is necessary, i.e. if any of:
    //   * ABR is not supported
    //   * the current config is Recovery
    //   * the current config status is Healthy
    NoOp,
    Active {
        current_config: ConfigurationWithoutRecovery,
        // None if the value is erroneously missing from QueryConfigurationStatusAndBootAttempts.
        boot_attempts: Option<u8>,
    },
}

impl PolicyEngine {
    /// Gathers system state from the BootManager.
    pub async fn build(boot_manager: &paver::BootManagerProxy) -> Result<Self, PolicyError> {
        let current_config = match boot_manager
            .query_current_configuration()
            .await
            .into_boot_manager_result("query_current_configuration")
        {
            Err(BootManagerError::Fidl {
                error: fidl::Error::ClientChannelClosed { epitaph, .. },
                ..
            }) if epitaph == Status::NOT_SUPPORTED => {
                info!("ABR not supported: skipping health verification and boot metadata updates");
                return Ok(Self(State::NoOp));
            }
            Err(e) => return Err(PolicyError::Build(e)),
            Ok(paver::Configuration::Recovery) => {
                info!("System in recovery: skipping health verification and boot metadata updates");
                return Ok(Self(State::NoOp));
            }
            Ok(paver::Configuration::A) => ConfigurationWithoutRecovery::A,
            Ok(paver::Configuration::B) => ConfigurationWithoutRecovery::B,
        };

        let status_and_boot_attempts = boot_manager
            .query_configuration_status_and_boot_attempts((&current_config).into())
            .await
            .into_boot_manager_result("query_configuration_status")
            .map_err(PolicyError::Build)?;
        match status_and_boot_attempts
            .status
            .ok_or(PolicyError::Build(BootManagerError::StatusNotSet))?
        {
            paver::ConfigurationStatus::Healthy => {
                return Ok(Self(State::NoOp));
            }
            paver::ConfigurationStatus::Pending => {}
            paver::ConfigurationStatus::Unbootable => {
                return Err(PolicyError::CurrentConfigurationUnbootable(
                    (&current_config).into(),
                    status_and_boot_attempts.unbootable_reason,
                ));
            }
        };

        let boot_attempts = status_and_boot_attempts.boot_attempts;
        if boot_attempts.is_none() {
            warn!("Current config status is pending but boot attempts was not set");
        }

        Ok(Self(State::Active { current_config, boot_attempts }))
    }

    /// Determines if we should verify and commit.
    /// * If we should (e.g. if the system is pending commit), return
    ///   `Some((slot_to_act_on, boot_attempts))`.
    /// * If we shouldn't (e.g. if the system is already committed), return `None`.
    pub fn should_verify_and_commit(&self) -> Option<(&ConfigurationWithoutRecovery, Option<u8>)> {
        match &self.0 {
            State::Active { current_config, boot_attempts } => {
                Some((current_config, *boot_attempts))
            }
            State::NoOp => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use mock_paver::{MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
    use std::sync::Arc;

    /// Test we should NOT verify and commit when when the device is in recovery.
    #[fuchsia::test]
    async fn test_skip_when_device_in_recovery() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(paver::Configuration::Recovery)
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((paver::ConfigurationStatus::Healthy, None))
                }))
                .build(),
        );
        let engine = PolicyEngine::build(&paver.spawn_boot_manager_service()).await.unwrap();

        assert_eq!(engine.should_verify_and_commit(), None);

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration]);
    }

    /// Test we should NOT verify and commit when the device does not support ABR.
    #[fuchsia::test]
    async fn test_skip_when_device_does_not_support_abr() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(Status::NOT_SUPPORTED)
                .build(),
        );
        let engine = PolicyEngine::build(&paver.spawn_boot_manager_service()).await.unwrap();

        assert_eq!(engine.should_verify_and_commit(), None);

        assert_eq!(paver.take_events(), vec![]);
    }

    /// Helper fn to verify we should NOT verify and commit when current is healthy.
    async fn test_skip_when_current_is_healthy(current_config: paver::Configuration) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config)
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((paver::ConfigurationStatus::Healthy, None))
                }))
                .build(),
        );
        let engine = PolicyEngine::build(&paver.spawn_boot_manager_service()).await.unwrap();

        assert_eq!(engine.should_verify_and_commit(), None);

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatusAndBootAttempts {
                    configuration: current_config
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn test_skip_when_current_is_healthy_a() {
        test_skip_when_current_is_healthy(paver::Configuration::A).await
    }

    #[fuchsia::test]
    async fn test_skip_when_current_is_healthy_b() {
        test_skip_when_current_is_healthy(paver::Configuration::B).await
    }

    /// Helper fn to verify we should verify and commit when current is pending.
    async fn test_verify_and_commit_when_current_is_pending(
        current_config: &ConfigurationWithoutRecovery,
    ) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config.into())
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((paver::ConfigurationStatus::Pending, Some(1)))
                }))
                .build(),
        );
        let engine = PolicyEngine::build(&paver.spawn_boot_manager_service()).await.unwrap();

        assert_eq!(engine.should_verify_and_commit(), Some((current_config, Some(1))));

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatusAndBootAttempts {
                    configuration: current_config.into()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn test_verify_and_commit_when_current_is_pending_a() {
        test_verify_and_commit_when_current_is_pending(&ConfigurationWithoutRecovery::A).await
    }

    #[fuchsia::test]
    async fn test_verify_and_commit_when_current_is_pending_b() {
        test_verify_and_commit_when_current_is_pending(&ConfigurationWithoutRecovery::B).await
    }

    /// Helper fn to verify an error is returned if current is unbootable.
    async fn test_returns_error_when_current_unbootable(
        current_config: &ConfigurationWithoutRecovery,
    ) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config.into())
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((paver::ConfigurationStatus::Unbootable, None))
                }))
                .build(),
        );

        assert_matches!(
            PolicyEngine::build(&paver.spawn_boot_manager_service()).await,
            Err(PolicyError::CurrentConfigurationUnbootable(cc, _)) if cc == current_config.into()
        );

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatusAndBootAttempts {
                    configuration: current_config.into()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn test_returns_error_when_current_unbootable_a() {
        test_returns_error_when_current_unbootable(&ConfigurationWithoutRecovery::A).await
    }

    #[fuchsia::test]
    async fn test_returns_error_when_current_unbootable_b() {
        test_returns_error_when_current_unbootable(&ConfigurationWithoutRecovery::B).await
    }

    /// Test the build fn fails on a standard paver error.
    #[fuchsia::test]
    async fn test_build_fails_when_paver_fails() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::return_error(|e| match e {
                    PaverEvent::QueryCurrentConfiguration => Err(Status::OUT_OF_RANGE),
                    _ => Ok(()),
                }))
                .build(),
        );

        assert_matches!(
            PolicyEngine::build(&paver.spawn_boot_manager_service()).await,
            Err(PolicyError::Build(BootManagerError::Status {
                method_name: "query_current_configuration",
                status: Status::OUT_OF_RANGE
            }))
        );

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration]);
    }
}
