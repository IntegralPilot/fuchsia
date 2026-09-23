// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use assert_matches::assert_matches;
use fidl_fuchsia_update_installer_ext::{
    Progress, State, StateId, UpdateInfo, UpdateInfoAndProgress, monitor_update,
};
use pretty_assertions::assert_eq;
use test_case::test_case;

#[fuchsia::test]
async fn progress_reporting_fetch_multiple_packages() {
    let env = TestEnv::builder().build().await;

    let pkg1_url = pinned_pkg_url!("package1/0", "aa");
    let pkg2_url = pinned_pkg_url!("package2/0", "bb");
    let pkg3_url = pinned_pkg_url!("package3/0", "cc");

    let update_pkg = env
        .resolver
        .package("update", UPDATE_HASH)
        .add_file("packages.json", make_packages_json([pkg1_url, pkg2_url, pkg3_url]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());
    let pkg1 = env.resolver.package("package1", merkle_str!("aa"));
    let pkg2 = env.resolver.package("package2", merkle_str!("bb"));
    let pkg3 = env.resolver.package("package3", merkle_str!("cc"));

    // We need to block all the resolves so that we can assert Fetch progress
    // is emitted for each pkg fetch. Otherwise, the Fetch state updates could merge
    // into one Fetch state.
    let handle_update_pkg = env.resolver.url(UPDATE_PKG_URL).block_once();
    let handle_pkg1 = env.resolver.url(pkg1_url).block_once();
    let handle_pkg2 = env.resolver.url(pkg2_url).block_once();
    let handle_pkg3 = env.resolver.url(pkg3_url).block_once();

    // Start the system update.
    let mut attempt = env.start_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);

    let info = UpdateInfo::builder().download_size(0).build();
    handle_update_pkg.resolve(&update_pkg).await;
    assert_eq!(attempt.next().await.unwrap().unwrap().id(), StateId::Stage);
    assert_eq!(attempt.next().await.unwrap().unwrap().id(), StateId::Stage);

    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Fetch(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(Progress::builder().fraction_completed(0.25).bytes_downloaded(0).build())
                .build()
        )
    );

    handle_pkg1.resolve(&pkg1).await;
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Fetch(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(Progress::builder().fraction_completed(0.5).bytes_downloaded(0).build())
                .build()
        )
    );

    handle_pkg2.resolve(&pkg2).await;
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Fetch(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(Progress::builder().fraction_completed(0.75).bytes_downloaded(0).build())
                .build()
        )
    );

    handle_pkg3.resolve(&pkg3).await;
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Fetch(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(Progress::builder().fraction_completed(1.0).bytes_downloaded(0).build())
                .build()
        )
    );

    // In this test, we are testing Fetch updates. Let's assert the Fetch
    // phase is over.
    assert_eq!(attempt.next().await.unwrap().unwrap().id(), StateId::Commit);
}

#[fuchsia::test]
async fn progress_reporting_fetch_multiple_blobs_packageless() {
    let image_content = vec![1; 500];
    let image_hash = fuchsia_merkle::root_from_slice(&image_content);

    let mut manifest = make_manifest([
        manifest::Blob { uncompressed_size: 100, fuchsia_merkle_root: hash(1) },
        manifest::Blob { uncompressed_size: 20, fuchsia_merkle_root: hash(2) },
        manifest::Blob { uncompressed_size: 3000, fuchsia_merkle_root: hash(3) },
    ]);
    manifest.images = vec![manifest::Image {
        slot: manifest::Slot::AB,
        image_type: manifest::ImageType::Asset(AssetType::Zbi),
        blob: manifest::Blob { uncompressed_size: 500, fuchsia_merkle_root: image_hash },
    }];

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(image_hash, image_content.clone())
        .build()
        .await;

    let handle_ota_manifest = env.http_loader_service.block_once();
    let handle_image_blob = env.ota_downloader_service.block_once(image_hash);
    let handle_blob1 = env.ota_downloader_service.block_once(hash(1));
    let handle_blob2 = env.ota_downloader_service.block_once(hash(2));
    let handle_blob3 = env.ota_downloader_service.block_once(hash(3));

    // Start the system update.
    let mut attempt = env.start_packageless_update().await.unwrap();

    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);

    let info = UpdateInfo::builder().download_size(0).build();
    handle_ota_manifest.await.unwrap().send(()).unwrap();

    let manifest_size = env.ota_manifest_size() as u64;
    let total_size = manifest_size as f32 + 4120.0;
    let mut total_downloaded_progress = manifest_size as f32;
    let mut total_bytes_downloaded = manifest_size;
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(
            UpdateInfoAndProgress::builder().info(info).progress(Progress::none()).build()
        )
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(
                    Progress::builder()
                        .fraction_completed(total_downloaded_progress / total_size)
                        .bytes_downloaded(total_bytes_downloaded)
                        .build()
                )
                .build()
        )
    );

    let sender = handle_image_blob.await.unwrap();
    let () = env.blobfs.write_blob(image_hash, &image_content).await.unwrap();
    sender.send(Ok(500)).unwrap();
    total_downloaded_progress += 1000.0;
    total_bytes_downloaded += 500;

    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(
                    Progress::builder()
                        .fraction_completed(total_downloaded_progress / total_size)
                        .bytes_downloaded(total_bytes_downloaded)
                        .build()
                )
                .build()
        )
    );

    assert_eq!(attempt.next().await.unwrap().unwrap().id(), StateId::Fetch);

    let mut remaining_blobs = vec![
        async move { (100.0, 100u64, handle_blob1.await) }.boxed(),
        async move { (20.0, 20u64, handle_blob2.await) }.boxed(),
        async move { (3000.0, 3000u64, handle_blob3.await) }.boxed(),
    ];

    while !remaining_blobs.is_empty() {
        let ((size, blob_bytes, sender), _index, remaining) =
            futures::future::select_all(remaining_blobs).await;
        sender.unwrap().send(Ok(blob_bytes)).unwrap();
        total_downloaded_progress += size;
        total_bytes_downloaded += blob_bytes;

        assert_eq!(
            attempt.next().await.unwrap().unwrap(),
            State::Fetch(
                UpdateInfoAndProgress::builder()
                    .info(info)
                    .progress(
                        Progress::builder()
                            .fraction_completed(total_downloaded_progress / total_size)
                            .bytes_downloaded(total_bytes_downloaded)
                            .build()
                    )
                    .build()
            )
        );
        remaining_blobs = remaining;
    }

    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Commit(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(
                    Progress::builder()
                        .fraction_completed(1.0)
                        .bytes_downloaded(total_bytes_downloaded)
                        .build()
                )
                .build()
        )
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::WaitToReboot(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(
                    Progress::builder()
                        .fraction_completed(1.0)
                        .bytes_downloaded(total_bytes_downloaded)
                        .build()
                )
                .build()
        )
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Reboot(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(
                    Progress::builder()
                        .fraction_completed(1.0)
                        .bytes_downloaded(total_bytes_downloaded)
                        .build()
                )
                .build()
        )
    );
}

#[fuchsia::test]
async fn monitor_fails_when_no_update_running() {
    let env = TestEnv::builder().build().await;

    // There is no update underway, so the monitor should not attach.
    assert_matches!(monitor_update(None, &env.installer_proxy()).await, Ok(None));
    assert_eq!(env.metric_event_logger_factory.clone_loggers().len(), 0);
    assert_eq!(env.take_interactions(), vec![]);
}

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn monitor_connects_to_existing_attempt(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    let update_pkg = env
        .resolver
        .package("update", UPDATE_HASH)
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("packages.json", make_packages_json([]))
        .add_file("images.json", make_images_json_zbi());

    // Block the update pkg resolve to ensure the update attempt is still in
    // in progress when we try to attach a monitor.
    let handle_update_pkg = env.resolver.url(UPDATE_PKG_URL).block_once();
    let handle_ota_manifest = env.http_loader_service.block_once();

    // Start the system update.
    let attempt0 =
        env.start_update_with_options(update_url, default_options(), None).await.unwrap();

    // Attach monitor.
    let attempt1 =
        monitor_update(Some(attempt0.attempt_id()), &env.installer_proxy()).await.unwrap().unwrap();

    // Now that we attached both monitors to the current attempt, we can unblock the
    // resolve and resume the update attempt.
    if update_url == UPDATE_PKG_URL {
        handle_update_pkg.resolve(&update_pkg).await;
    } else {
        handle_ota_manifest.await.unwrap().send(()).unwrap();
    }
    let monitor0_events: Vec<State> = attempt0.map(|res| res.unwrap()).collect().await;
    let monitor1_events: Vec<State> = attempt1.map(|res| res.unwrap()).collect().await;

    // Since we wait until the update attempt is over to read from monitor1 events,
    // we should expect that the events in monitor1 merged.
    assert_eq!(monitor1_events.len(), 6);

    // While the number of events are different, the ordering should still be the same.
    let expected_order = [
        StateId::Prepare,
        StateId::Stage,
        StateId::Fetch,
        StateId::Commit,
        StateId::WaitToReboot,
        StateId::Reboot,
    ];
    assert_success_monitor_states(monitor0_events, &expected_order);
    assert_success_monitor_states(monitor1_events, &expected_order);
}

#[test_case(UPDATE_PKG_URL)]
#[test_case(MANIFEST_URL)]
#[fuchsia::test]
async fn succeed_additional_start_requests_when_compatible(update_url: &str) {
    let env = TestEnv::builder().ota_manifest(make_manifest([])).build().await;

    let update_pkg = env
        .resolver
        .package("update", UPDATE_HASH)
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", make_images_json_zbi());

    // Block the update pkg resolve to ensure the update attempt is still in
    // in progress when we try to attach a monitor.
    let handle_update_pkg = env.resolver.url(UPDATE_PKG_URL).block_once();
    let handle_ota_manifest = env.http_loader_service.block_once();

    // Start the system update, making 2 start_update requests. The second start_update request
    // is essentially just a monitor_update request in this case.
    let attempt0 =
        env.start_update_with_options(update_url, default_options(), None).await.unwrap();
    let attempt1 = env
        .start_update_with_options(
            update_url,
            Options {
                initiator: Initiator::User,
                allow_attach_to_existing_attempt: true,
                should_write_recovery: true,
                manifest_range: None,
                manifest_headers: vec![],
            },
            None,
        )
        .await
        .unwrap();

    // Now that we attached both monitors to the current attempt, we can unblock the
    // resolve and resume the update attempt.
    if update_url == UPDATE_PKG_URL {
        handle_update_pkg.resolve(&update_pkg).await;
    } else {
        handle_ota_manifest.await.unwrap().send(()).unwrap();
    }
    let monitor0_events: Vec<State> = attempt0.map(|res| res.unwrap()).collect().await;
    let monitor1_events: Vec<State> = attempt1.map(|res| res.unwrap()).collect().await;

    // Since we waited for the update attempt to complete before reading from monitor1,
    // the events in monitor1 should have merged.
    assert_eq!(monitor1_events.len(), 6);

    // While the number of events are different, the ordering should still be the same.
    let expected_order = [
        StateId::Prepare,
        StateId::Stage,
        StateId::Fetch,
        StateId::Commit,
        StateId::WaitToReboot,
        StateId::Reboot,
    ];
    assert_success_monitor_states(monitor0_events, &expected_order);
    assert_success_monitor_states(monitor1_events, &expected_order);
}

#[test_case(UPDATE_PKG_URL, "fuchsia-pkg://fuchsia.com/different-url")]
#[test_case(MANIFEST_URL, "http://fuchsia.com/different-url")]
#[fuchsia::test]
async fn fail_additional_start_requests_when_not_compatible(
    compatible_url: &str,
    incompatible_url: &str,
) {
    let env = TestEnv::builder().build().await;

    env.resolver
        .package("update", UPDATE_HASH)
        .add_file("packages.json", make_packages_json([]))
        .add_file("images.json", make_images_json_zbi());

    // Block the update pkg resolve to ensure the update attempt is still in
    // in progress when we try to make additional start_update requests.
    let _handle_update_pkg = env.resolver.url(UPDATE_PKG_URL).block_once();
    let _handle_ota_manifest = env.http_loader_service.block_once();

    // Start the system update.
    let compatible_options = Options {
        initiator: Initiator::User,
        allow_attach_to_existing_attempt: true,
        should_write_recovery: true,
        manifest_range: None,
        manifest_headers: vec![],
    };
    let _attempt = env
        .start_update_with_options(compatible_url, compatible_options.clone(), None)
        .await
        .unwrap();

    // Define incompatible options and url.
    let incompatible_options0 = Options {
        initiator: Initiator::User,
        allow_attach_to_existing_attempt: true,
        should_write_recovery: false,
        manifest_range: None,
        manifest_headers: vec![],
    };
    let incompatible_options1 = Options {
        initiator: Initiator::User,
        allow_attach_to_existing_attempt: false,
        should_write_recovery: true,
        manifest_range: None,
        manifest_headers: vec![],
    };

    // Show that start_update requests fail with AlreadyInProgress errors.
    assert_matches!(
        env.start_update_with_options(compatible_url, incompatible_options0, None)
            .await
            .map(|_| ())
            .unwrap_err(),
        UpdateAttemptError::InstallInProgress
    );
    assert_matches!(
        env.start_update_with_options(compatible_url, incompatible_options1, None)
            .await
            .map(|_| ())
            .unwrap_err(),
        UpdateAttemptError::InstallInProgress
    );
    assert_matches!(
        env.start_update_with_options(incompatible_url, compatible_options.clone(), None)
            .await
            .map(|_| ())
            .unwrap_err(),
        UpdateAttemptError::InstallInProgress
    );
    let (_, server_end) = fidl::endpoints::create_endpoints();
    assert_matches!(
        env.start_update_with_options(compatible_url, compatible_options, Some(server_end))
            .await
            .map(|_| ())
            .unwrap_err(),
        UpdateAttemptError::InstallInProgress
    );
}

pub fn assert_success_monitor_states(states: Vec<State>, ordering: &[StateId]) {
    let res = util::verify_monitor_states(&states, ordering, false);
    if let Err(e) = res {
        panic!(
            "Error received when verifying monitor states: {e:#}\nWant ordering: {ordering:#?}\nGot states:{states:#?}"
        );
    }
}

mod util {
    use fidl_fuchsia_update_installer_ext::{
        PrepareFailureReason, Progress, State, StateId, UpdateInfo, UpdateInfoAndProgress,
    };
    use std::collections::HashSet;
    use thiserror::Error;

    #[derive(Debug, Error, PartialEq)]
    pub enum VerifyMonitorStatesError {
        #[error("there are more IDs in the ordering than in the provided states")]
        TooFewStates,

        #[error("the order of the states does not match the passed in ordering")]
        OutOfOrder,

        #[error("progress should be strictly nondecreasing")]
        ProgressDecreased,

        #[error("received a {0:?} state, which wasn't in the ordering")]
        UnexpectedState(StateId),

        #[error("the final fraction_completed should be 1.0 on successful attempts")]
        FractionCompletedSuccessNot1,
    }

    /// Validate that
    /// * states are in the right order (ignoring duplicates)
    /// * progress is strictly nondecreasing
    /// * fraction_completed stays within [0.0, 1,0] bounds
    /// * on successful update attempts, the final progress should be 1.0.
    pub fn verify_monitor_states(
        states: &[State],
        ordering: &[StateId],
        expect_success: bool,
    ) -> Result<(), VerifyMonitorStatesError> {
        // Sanity check input
        if states.len() < ordering.len() {
            return Err(VerifyMonitorStatesError::TooFewStates);
        }
        let ordering_set: HashSet<StateId> = ordering.iter().cloned().collect();
        if ordering_set.len() != ordering.len() {
            panic!("Ordering should not have duplicates: {ordering:?} ");
        }
        for state in states.iter() {
            if !ordering.contains(&state.id()) {
                return Err(VerifyMonitorStatesError::UnexpectedState(state.id()));
            }
        }

        let mut prev_fraction_completed = 0.0;
        let mut state_index = 0;
        for item in ordering {
            // Check if it's out of order.
            if states[state_index].id() != *item {
                return Err(VerifyMonitorStatesError::OutOfOrder);
            }

            // Check progress.
            while state_index < states.len() && states[state_index].id() == *item {
                if let Some(progress) = states[state_index].progress() {
                    // Verify we aren't decreasing.
                    if progress.fraction_completed() < prev_fraction_completed {
                        return Err(VerifyMonitorStatesError::ProgressDecreased);
                    }
                    prev_fraction_completed = progress.fraction_completed();
                }
                state_index += 1;
            }
        }

        // The last progress should be 1.0 on success.
        if expect_success {
            let states_with_full_fraction_completion = states.iter().find(|state| {
                if let Some(progress) = state.progress() {
                    return progress.fraction_completed() == 1.0;
                }
                false
            });
            if states_with_full_fraction_completion.is_none() {
                return Err(VerifyMonitorStatesError::FractionCompletedSuccessNot1);
            }
        }

        Ok(())
    }

    #[test]
    fn fail_too_few_states() {
        let states = vec![State::Prepare];
        let ordering = vec![StateId::Prepare, StateId::Stage];

        assert_eq!(
            verify_monitor_states(&states, &ordering, true),
            Err(VerifyMonitorStatesError::TooFewStates)
        );
    }

    #[test]
    #[should_panic]
    // TODO(https://fxbug.dev/42169733): delete the below
    #[cfg_attr(feature = "variant_asan", ignore)]
    #[cfg_attr(feature = "variant_hwasan", ignore)]
    fn fail_duplicates_in_ordering() {
        let states = vec![State::Prepare, State::Prepare];
        let ordering = vec![StateId::Prepare, StateId::Prepare];

        verify_monitor_states(&states, &ordering, true).unwrap();
    }

    #[test]
    fn fail_unexpected_state() {
        let states = vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)];
        let ordering = vec![StateId::Prepare];

        assert_eq!(
            verify_monitor_states(&states, &ordering, true),
            Err(VerifyMonitorStatesError::UnexpectedState(StateId::FailPrepare))
        );
    }

    #[test]
    fn fail_out_of_order() {
        let states = vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal)];
        let ordering = vec![StateId::FailPrepare, StateId::Prepare];

        assert_eq!(
            verify_monitor_states(&states, &ordering, true),
            Err(VerifyMonitorStatesError::OutOfOrder)
        );
    }

    #[test]
    fn fail_progress_decreased() {
        let info = UpdateInfo::builder().download_size(0).build();
        let d0 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build())
            .build();
        let d1 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.4).bytes_downloaded(0).build())
            .build();
        let d2 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.2).bytes_downloaded(0).build())
            .build();
        let states = vec![State::Prepare, State::Fetch(d0), State::Fetch(d1), State::Fetch(d2)];
        let ordering = vec![StateId::Prepare, StateId::Fetch];

        assert_eq!(
            verify_monitor_states(&states, &ordering, true),
            Err(VerifyMonitorStatesError::ProgressDecreased)
        );
    }

    #[test]
    fn fail_fraction_completed_should_end_with_1_on_success() {
        let info = UpdateInfo::builder().download_size(0).build();
        let d0 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build())
            .build();
        let d1 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.9).bytes_downloaded(0).build())
            .build();
        let states = vec![State::Prepare, State::Fetch(d0), State::Fetch(d1)];
        let ordering = vec![StateId::Prepare, StateId::Fetch];

        assert_eq!(
            verify_monitor_states(&states, &ordering, true),
            Err(VerifyMonitorStatesError::FractionCompletedSuccessNot1)
        );
        // Sanity check failure method doesn't care if last fraction completed is not 1.0.
        assert_eq!(verify_monitor_states(&states, &ordering, false), Ok(()));
    }

    #[test]
    fn success() {
        let info = UpdateInfo::builder().download_size(0).build();
        let d0 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build())
            .build();
        let d1 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(0.5).bytes_downloaded(0).build())
            .build();
        let d2 = UpdateInfoAndProgress::builder()
            .info(info)
            .progress(Progress::builder().fraction_completed(1.0).bytes_downloaded(0).build())
            .build();
        let states = vec![
            State::Prepare,
            State::Fetch(d0),
            State::Fetch(d1),
            State::Stage(d1),
            State::Stage(d2),
            State::Reboot(d2),
        ];
        let ordering = vec![StateId::Prepare, StateId::Fetch, StateId::Stage, StateId::Reboot];

        assert_eq!(verify_monitor_states(&states, &ordering, true), Ok(()));
    }
}
