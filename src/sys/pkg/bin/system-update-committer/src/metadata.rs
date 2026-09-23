// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Handles interfacing with the boot metadata (e.g. verifying a slot, committing a slot, etc).

use commit::do_commit;
use errors::MetadataError;
use fidl_fuchsia_paver as fpaver;
use fidl_fuchsia_update_verify::HealthVerificationProxy;
use fuchsia_async::TimeoutExt as _;
use fuchsia_inspect as finspect;
use futures::channel::oneshot;
use policy::PolicyEngine;
use std::time::Instant;
use zx::{EventPair, Peered};

mod commit;
mod configuration_without_recovery;
mod errors;
mod inspect;
mod policy;

/// Puts BootManager metadata into a happy state, provided we believe the system can OTA.
///
/// The "happy state" is:
/// * The current configuration is active and marked Healthy.
/// * The alternate configuration is marked Unbootable.
///
/// To put the metadata in this state, we may need to verify and commit. To make it easier to
/// determine if we should verify and commit, we consult the `PolicyEngine`.
///
/// If this function returns an error, it likely means that the system is somehow busted, and that
/// it should be rebooted. Rebooting will hopefully either fix the issue or decrement the boot
/// counter, eventually leading to a rollback.
pub async fn put_metadata_in_happy_state(
    boot_manager: &fpaver::BootManagerProxy,
    p_internal: &EventPair,
    unblocker: oneshot::Sender<()>,
    health_verification: &HealthVerificationProxy,
    commit_timeout: zx::MonotonicDuration,
    node: &finspect::Node,
    commit_inspect: &CommitInspect,
) -> Result<CommitResult, MetadataError> {
    let start_time = Instant::now();
    let res = put_metadata_in_happy_state_impl(
        boot_manager,
        p_internal,
        unblocker,
        health_verification,
        commit_inspect,
    )
    .on_timeout(commit_timeout, || Err(MetadataError::Timeout))
    .await;

    match &res {
        Ok(CommitResult::CommitNotNecessary) => (),
        Ok(CommitResult::CommittedSystem) | Err(_) => {
            inspect::write_to_inspect(node, res.as_ref().map(|_| ()), start_time.elapsed())
        }
    }

    res
}

async fn put_metadata_in_happy_state_impl(
    boot_manager: &fpaver::BootManagerProxy,
    p_internal: &EventPair,
    unblocker: oneshot::Sender<()>,
    health_verification: &HealthVerificationProxy,
    commit_inspect: &CommitInspect,
) -> Result<CommitResult, MetadataError> {
    let mut unblocker = Some(unblocker);
    let commit_result = {
        let engine = PolicyEngine::build(boot_manager).await.map_err(MetadataError::Policy)?;
        if let Some((current_config, boot_attempts)) = engine.should_verify_and_commit() {
            // At this point, the FIDL server should start responding to requests so that clients
            // can find out that the health verification is underway.
            unblocker = unblock_fidl_server(unblocker)?;
            let () =
                zx::Status::ok(health_verification.query_health_checks().await.map_err(|e| {
                    MetadataError::HealthVerification(errors::HealthVerificationError::Fidl(e))
                })?)
                .map_err(|e| {
                    MetadataError::HealthVerification(errors::HealthVerificationError::Unhealthy(e))
                })?;
            let () =
                do_commit(boot_manager, current_config).await.map_err(MetadataError::Commit)?;
            let () = commit_inspect.record_boot_attempts(boot_attempts);
            CommitResult::CommittedSystem
        } else {
            CommitResult::CommitNotNecessary
        }
    };

    // Tell the rest of the system we are now committed.
    let () = p_internal
        .signal_peer(zx::Signals::NONE, zx::Signals::USER_0)
        .map_err(MetadataError::SignalPeer)?;

    // Ensure the FIDL server will be unblocked, even if we didn't verify health.
    unblock_fidl_server(unblocker)?;

    Ok(commit_result)
}

#[derive(Debug)]
pub enum CommitResult {
    CommittedSystem,
    CommitNotNecessary,
}

impl CommitResult {
    pub fn log_msg(&self) -> &'static str {
        match self {
            Self::CommittedSystem => "Committed system.",
            Self::CommitNotNecessary => "Commit not necessary.",
        }
    }
}

/// Records inspect data specific to committing the update if the health checks pass.
pub struct CommitInspect(finspect::Node);

impl CommitInspect {
    pub fn new(node: finspect::Node) -> Self {
        Self(node)
    }

    fn record_boot_attempts(&self, count: Option<u8>) {
        match count {
            Some(count) => self.0.record_uint("boot_attempts", count.into()),
            None => self.0.record_uint("boot_attempts_missing", 0),
        }
    }
}

fn unblock_fidl_server(
    unblocker: Option<oneshot::Sender<()>>,
) -> Result<Option<oneshot::Sender<()>>, MetadataError> {
    if let Some(sender) = unblocker {
        let () = sender.send(()).map_err(|_| MetadataError::Unblock)?;
    }
    Ok(None)
}

// There is intentionally some overlap between the tests here and in `policy`. We do this so we can
// test the functionality at different layers.
#[cfg(test)]
mod tests {
    use super::errors::HealthVerificationError;
    use super::*;
    use assert_matches::assert_matches;
    use configuration_without_recovery::ConfigurationWithoutRecovery;
    use fasync::OnSignals;
    use fuchsia_async as fasync;
    use mock_health_verification::MockHealthVerificationService;
    use mock_paver::{MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use zx::Status;

    fn health_verification_and_call_count(
        status: Result<(), zx::Status>,
    ) -> (HealthVerificationProxy, Arc<AtomicU32>) {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = Arc::clone(&call_count);
        let verification = Arc::new(MockHealthVerificationService::new(move || {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            status
        }));

        let (health_verification, server) = verification.spawn_health_verification_service();
        let () = server.detach();

        (health_verification, call_count)
    }

    /// When we don't support ABR, we should not update metadata.
    /// However, the FIDL server should still be unblocked.
    #[fuchsia::test]
    async fn test_does_not_change_metadata_when_device_does_not_support_abr() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(Status::NOT_SUPPORTED)
                .build(),
        );
        let (p_internal, p_external) = EventPair::create();
        let (unblocker, unblocker_recv) = oneshot::channel();
        let (health_verification, health_verification_call_count) =
            health_verification_and_call_count(Ok(()));

        put_metadata_in_happy_state_impl(
            &paver.spawn_boot_manager_service(),
            &p_internal,
            unblocker,
            &health_verification,
            &CommitInspect::new(finspect::Node::default()),
        )
        .await
        .unwrap();

        assert_eq!(paver.take_events(), vec![]);
        assert_eq!(
            p_external
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                .to_result(),
            Ok(zx::Signals::USER_0)
        );
        assert_eq!(unblocker_recv.await, Ok(()));
        assert_eq!(health_verification_call_count.load(Ordering::SeqCst), 0);
    }

    /// When we're in recovery, we should not update metadata.
    /// However, the FIDL server should still be unblocked.
    #[fuchsia::test]
    async fn test_does_not_change_metadata_when_device_in_recovery() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(fpaver::Configuration::Recovery)
                .insert_hook(mphooks::config_status(|_| Ok(fpaver::ConfigurationStatus::Healthy)))
                .build(),
        );
        let (p_internal, p_external) = EventPair::create();
        let (unblocker, unblocker_recv) = oneshot::channel();
        let (health_verification, health_verification_call_count) =
            health_verification_and_call_count(Ok(()));

        put_metadata_in_happy_state_impl(
            &paver.spawn_boot_manager_service(),
            &p_internal,
            unblocker,
            &health_verification,
            &CommitInspect::new(finspect::Node::default()),
        )
        .await
        .unwrap();

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration]);
        assert_eq!(
            p_external
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                .to_result(),
            Ok(zx::Signals::USER_0)
        );
        assert_eq!(unblocker_recv.await, Ok(()));
        assert_eq!(health_verification_call_count.load(Ordering::SeqCst), 0);
    }

    /// When the current slot is healthy, we should not update metadata.
    /// However, the FIDL server should still be unblocked.
    async fn test_does_not_change_metadata_when_current_is_healthy(
        current_config: &ConfigurationWithoutRecovery,
    ) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config.into())
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((fpaver::ConfigurationStatus::Healthy, None))
                }))
                .build(),
        );
        let (p_internal, p_external) = EventPair::create();
        let (unblocker, unblocker_recv) = oneshot::channel();
        let (health_verification, health_verification_call_count) =
            health_verification_and_call_count(Ok(()));

        put_metadata_in_happy_state_impl(
            &paver.spawn_boot_manager_service(),
            &p_internal,
            unblocker,
            &health_verification,
            &CommitInspect::new(finspect::Node::default()),
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatusAndBootAttempts {
                    configuration: current_config.into()
                }
            ]
        );
        assert_eq!(
            p_external
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                .to_result(),
            Ok(zx::Signals::USER_0)
        );
        assert_eq!(unblocker_recv.await, Ok(()));
        assert_eq!(health_verification_call_count.load(Ordering::SeqCst), 0);
    }

    #[fuchsia::test]
    async fn test_does_not_change_metadata_when_current_is_healthy_a() {
        test_does_not_change_metadata_when_current_is_healthy(&ConfigurationWithoutRecovery::A)
            .await
    }

    #[fuchsia::test]
    async fn test_does_not_change_metadata_when_current_is_healthy_b() {
        test_does_not_change_metadata_when_current_is_healthy(&ConfigurationWithoutRecovery::B)
            .await
    }

    /// When the current slot is pending, we should verify, commit, & unblock the fidl server.
    async fn test_verifies_and_commits_when_current_is_pending(
        current_config: &ConfigurationWithoutRecovery,
    ) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config.into())
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((fpaver::ConfigurationStatus::Pending, Some(1)))
                }))
                .build(),
        );
        let (p_internal, p_external) = EventPair::create();
        let (unblocker, unblocker_recv) = oneshot::channel();
        let (health_verification, health_verification_call_count) =
            health_verification_and_call_count(Ok(()));

        put_metadata_in_happy_state_impl(
            &paver.spawn_boot_manager_service(),
            &p_internal,
            unblocker,
            &health_verification,
            &CommitInspect::new(finspect::Node::default()),
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatusAndBootAttempts {
                    configuration: current_config.into()
                },
                PaverEvent::SetConfigurationHealthy { configuration: current_config.into() },
                PaverEvent::SetConfigurationUnbootable {
                    configuration: current_config.to_alternate().into()
                },
                PaverEvent::BootManagerFlush,
            ]
        );
        assert_eq!(OnSignals::new(&p_external, zx::Signals::USER_0).await, Ok(zx::Signals::USER_0));
        assert_eq!(unblocker_recv.await, Ok(()));
        assert_eq!(health_verification_call_count.load(Ordering::SeqCst), 1);
    }

    #[fuchsia::test]
    async fn test_verifies_and_commits_when_current_is_pending_a() {
        test_verifies_and_commits_when_current_is_pending(&ConfigurationWithoutRecovery::A).await
    }

    #[fuchsia::test]
    async fn test_verifies_and_commits_when_current_is_pending_b() {
        test_verifies_and_commits_when_current_is_pending(&ConfigurationWithoutRecovery::B).await
    }

    /// When we fail to verify and the config says to ignore, we should still do the commit.
    #[fuchsia::test]
    async fn test_commits_when_failed_verification_ignored() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(fpaver::Configuration::Recovery)
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((fpaver::ConfigurationStatus::Pending, Some(1)))
                }))
                .build(),
        );
        let (p_internal, p_external) = EventPair::create();
        let (unblocker, unblocker_recv) = oneshot::channel();
        let (health_verification, health_verification_call_count) =
            health_verification_and_call_count(Err(zx::Status::INTERNAL));

        put_metadata_in_happy_state_impl(
            &paver.spawn_boot_manager_service(),
            &p_internal,
            unblocker,
            &health_verification,
            &CommitInspect::new(finspect::Node::default()),
        )
        .await
        .unwrap();

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration,]);
        assert_eq!(OnSignals::new(&p_external, zx::Signals::USER_0).await, Ok(zx::Signals::USER_0));
        assert_eq!(unblocker_recv.await, Ok(()));
        assert_eq!(health_verification_call_count.load(Ordering::SeqCst), 0);
    }

    /// When we fail to verify and the config says to not ignore, we should report an error.
    async fn test_errors_when_failed_verification_not_ignored(
        current_config: &ConfigurationWithoutRecovery,
    ) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config.into())
                .insert_hook(mphooks::config_status_and_boot_attempts(|_| {
                    Ok((fpaver::ConfigurationStatus::Pending, Some(1)))
                }))
                .build(),
        );
        let (p_internal, p_external) = EventPair::create();
        let (unblocker, unblocker_recv) = oneshot::channel();
        let (health_verification, health_verification_call_count) =
            health_verification_and_call_count(Err(zx::Status::INTERNAL));

        let result = put_metadata_in_happy_state_impl(
            &paver.spawn_boot_manager_service(),
            &p_internal,
            unblocker,
            &health_verification,
            &CommitInspect::new(finspect::Node::default()),
        )
        .await;

        assert_matches!(
            result,
            Err(MetadataError::HealthVerification(HealthVerificationError::Unhealthy(
                Status::INTERNAL
            )))
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
        assert_eq!(
            p_external
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                .to_result(),
            Err(zx::Status::TIMED_OUT)
        );
        assert_eq!(unblocker_recv.await, Ok(()));
        assert_eq!(health_verification_call_count.load(Ordering::SeqCst), 1);
    }

    #[fuchsia::test]
    async fn test_errors_when_failed_verification_not_ignored_a() {
        test_errors_when_failed_verification_not_ignored(&ConfigurationWithoutRecovery::A).await
    }

    #[fuchsia::test]
    async fn test_errors_when_failed_verification_not_ignored_b() {
        test_errors_when_failed_verification_not_ignored(&ConfigurationWithoutRecovery::B).await
    }

    #[fuchsia::test]
    async fn commit_inspect_handles_missing_count() {
        let inspector = finspect::Inspector::default();
        let commit_inspect = CommitInspect::new(inspector.root().create_child("commit"));

        commit_inspect.record_boot_attempts(None);

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            "commit": {
                "boot_attempts_missing": 0u64
            }
        });
    }
}
