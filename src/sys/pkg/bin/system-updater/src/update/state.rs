// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Wrappers around the installer state machine to track progress and ensure the installer can only
//! make valid state transitions.

use async_generator::Yield;
use fidl_fuchsia_update_installer_ext::{
    FetchFailureReason, PrepareFailureReason, Progress, StageFailureReason, State, StateId,
    UpdateInfo, UpdateInfoAndProgress,
};
use fuchsia_async as fasync;
use std::time::Duration;

/// Tracks a numeric goal and the current progress towards that goal, ensuring progress can only go
/// forwards and never exceeds 100%.
#[derive(Debug)]
struct ProgressTracker {
    goal: u64,
    current: u64,
    last_reported_fraction: f32,
    last_reported_time: fasync::BootInstant,
    last_reported_state_id: Option<StateId>,
}

impl ProgressTracker {
    fn new(goal: u64) -> Self {
        Self {
            goal,
            current: 0,
            last_reported_fraction: 0.0,
            last_reported_time: fasync::BootInstant::INFINITE_PAST,
            last_reported_state_id: None,
        }
    }

    fn add(&mut self, n: u64) {
        self.current += n;
        if self.current > self.goal {
            self.current = self.goal;
        }
    }

    fn done(&self) -> bool {
        self.goal == self.current
    }

    fn as_fraction(&self) -> f32 {
        if self.goal == 0 { 1.0 } else { self.current as f32 / self.goal as f32 }
    }

    /// Yields `state` if enough time or progress has elapsed since the last report,
    /// or if the goal or state has changed. Updates throttle timestamps when yielding.
    async fn yield_if_due(&mut self, co: &mut Yield<State>, state: State) {
        let now = fasync::BootInstant::now();
        let current_fraction = self.as_fraction();
        let delta_fraction = current_fraction - self.last_reported_fraction;
        let elapsed_time = now - self.last_reported_time;
        let state_id = Some(state.id());

        if self.last_reported_state_id == state_id
            && !self.done()
            && delta_fraction < 0.001
            && elapsed_time < Duration::from_millis(100).into()
        {
            return;
        }

        self.last_reported_fraction = current_fraction;
        self.last_reported_time = now;
        self.last_reported_state_id = state_id;
        co.yield_(state).await;
    }
}

/// The Prepare state.
#[must_use]
pub struct Prepare;

impl Prepare {
    /// Start at the Prepare state.
    pub async fn enter(co: &mut Yield<State>) -> Prepare {
        co.yield_(State::Prepare).await;
        Prepare
    }

    /// Transition to Stage state with the given update info and numeric progress target.
    ///
    /// The sum of all n given to [`Fetch::add_progress`] and [`Stage::add_progress`] should equal
    /// `progress_goal` specified here.
    pub async fn enter_stage(
        self,
        co: &mut Yield<State>,
        info: UpdateInfo,
        progress_goal: u64,
    ) -> Stage {
        co.yield_(State::Stage(
            UpdateInfoAndProgress::builder().info(info).progress(Progress::none()).build(),
        ))
        .await;

        Stage { info, bytes_downloaded: 0, progress: ProgressTracker::new(progress_goal) }
    }

    /// Transition to the FailPrepare terminal state.
    pub async fn fail(self, co: &mut Yield<State>, reason: PrepareFailureReason) {
        co.yield_(State::FailPrepare(reason)).await;
    }
}

/// The Stage state.
#[must_use]
pub struct Stage {
    info: UpdateInfo,
    bytes_downloaded: u64,
    progress: ProgressTracker,
}

impl Stage {
    fn progress(&self) -> Progress {
        Progress::builder()
            .fraction_completed(self.progress.as_fraction())
            .bytes_downloaded(self.bytes_downloaded)
            .build()
    }

    fn info_progress(&self) -> UpdateInfoAndProgress {
        UpdateInfoAndProgress::builder().info(self.info).progress(self.progress()).build()
    }

    /// Increment the progress by `progress` and bytes downloaded by `bytes` and emit a status
    /// update if not throttled.
    pub async fn add_progress(&mut self, co: &mut Yield<State>, progress: u64, bytes: u64) {
        self.progress.add(progress);
        self.bytes_downloaded = self.bytes_downloaded.saturating_add(bytes);
        self.progress.yield_if_due(co, State::Stage(self.info_progress())).await;
    }

    /// Transition to the Fetch state.
    pub async fn enter_fetch(mut self, co: &mut Yield<State>) -> Fetch {
        self.progress.yield_if_due(co, State::Fetch(self.info_progress())).await;
        Fetch { info: self.info, bytes_downloaded: self.bytes_downloaded, progress: self.progress }
    }

    /// Transition to the FailStage terminal state.
    pub async fn fail(self, co: &mut Yield<State>, reason: StageFailureReason) {
        co.yield_(State::FailStage(self.info_progress().with_stage_reason(reason))).await;
    }
}

/// The Fetch state.
#[must_use]
pub struct Fetch {
    info: UpdateInfo,
    bytes_downloaded: u64,
    progress: ProgressTracker,
}

impl Fetch {
    fn info(&self) -> UpdateInfo {
        self.info
    }

    fn progress(&self) -> Progress {
        Progress::builder()
            .fraction_completed(self.progress.as_fraction())
            .bytes_downloaded(self.bytes_downloaded)
            .build()
    }

    fn info_progress(&self) -> UpdateInfoAndProgress {
        UpdateInfoAndProgress::builder().info(self.info()).progress(self.progress()).build()
    }

    /// Increment the progress by `progress` and bytes downloaded by `bytes` and emit a status
    /// update if not throttled.
    pub async fn add_progress(&mut self, co: &mut Yield<State>, progress: u64, bytes: u64) {
        self.progress.add(progress);
        self.bytes_downloaded = self.bytes_downloaded.saturating_add(bytes);
        self.progress.yield_if_due(co, State::Fetch(self.info_progress())).await;
    }

    /// Transition to the Commit state.
    pub async fn enter_commit(self, co: &mut Yield<State>) -> Commit {
        debug_assert!(self.progress.done());
        co.yield_(State::Commit(self.info_progress())).await;
        Commit {
            info: self.info(),
            bytes_downloaded: self.bytes_downloaded,
            progress: self.progress,
        }
    }

    /// Transition to the FailFetch terminal state.
    pub async fn fail(self, co: &mut Yield<State>, reason: FetchFailureReason) {
        co.yield_(State::FailFetch(self.info_progress().with_fetch_reason(reason))).await;
    }
}

/// The Commit state.
#[must_use]
pub struct Commit {
    info: UpdateInfo,
    bytes_downloaded: u64,
    progress: ProgressTracker,
}

impl Commit {
    /// Transition to the WaitToReboot state.
    pub async fn enter_wait_to_reboot(self, co: &mut Yield<State>) -> WaitToReboot {
        co.yield_(State::WaitToReboot(self.info_progress())).await;
        WaitToReboot { info: self.info, bytes_downloaded: self.bytes_downloaded }
    }

    fn progress(&self) -> Progress {
        Progress::builder()
            .fraction_completed(self.progress.as_fraction())
            .bytes_downloaded(self.bytes_downloaded)
            .build()
    }

    fn info_progress(&self) -> UpdateInfoAndProgress {
        UpdateInfoAndProgress::builder().info(self.info).progress(self.progress()).build()
    }

    /// Transition to the FailCommit terminal state.
    pub async fn fail(self, co: &mut Yield<State>) {
        co.yield_(State::FailCommit(self.info_progress())).await;
    }
}

/// The WaitToReboot state.
#[must_use]
pub struct WaitToReboot {
    info: UpdateInfo,
    bytes_downloaded: u64,
}

impl WaitToReboot {
    fn progress(&self) -> Progress {
        Progress::builder().fraction_completed(1.0).bytes_downloaded(self.bytes_downloaded).build()
    }

    fn info_progress(&self) -> UpdateInfoAndProgress {
        UpdateInfoAndProgress::builder().info(self.info).progress(self.progress()).build()
    }

    /// Transition to the Reboot terminal state.
    pub async fn enter_reboot(self, co: &mut Yield<State>) {
        let state = State::Reboot(self.info_progress());
        co.yield_(state).await;
    }

    /// Transition to the DeferReboot terminal state.
    pub async fn enter_defer_reboot(self, co: &mut Yield<State>) {
        let state = State::DeferReboot(self.info_progress());
        co.yield_(state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::prelude::*;
    use std::assert_matches;
    use std::task::Poll;

    #[test]
    fn progress_no_goal_is_done() {
        assert!(ProgressTracker::new(0).done());
        assert_eq!(ProgressTracker::new(0).as_fraction(), 1.0);
    }

    #[test]
    fn progress_goal_of_one() {
        let mut progress = ProgressTracker::new(1);
        assert!(!progress.done());
        assert_eq!(progress.as_fraction(), 0.0);

        progress.add(1);
        assert!(progress.done());
        assert_eq!(progress.as_fraction(), 1.0);
    }

    #[test]
    fn progress_saturates_at_one() {
        let mut progress = ProgressTracker::new(2);
        progress.add(1);
        progress.add(3); // 200% done
        assert!(progress.done());
        assert_eq!(progress.as_fraction(), 1.0);
    }

    #[test]
    fn progress_increases() {
        let mut progress = ProgressTracker::new(100);
        let mut last = progress.as_fraction();

        for _ in 0..100 {
            progress.add(1);
            assert!(last < progress.as_fraction());
            last = progress.as_fraction();
        }
    }

    async fn collect_states<CB, FT>(cb: CB) -> Vec<State>
    where
        CB: FnOnce(Yield<State>) -> FT,
        FT: Future<Output = ()>,
    {
        async_generator::generate(cb).into_yielded().collect().await
    }

    #[fuchsia::test]
    async fn yields_expected_states_success() {
        let info = UpdateInfo::builder().download_size(0).build();

        assert_eq!(
            collect_states(|mut co| async move {
                let info = UpdateInfo::builder().download_size(0).build();
                let state = Prepare::enter(&mut co).await;
                let mut state = state.enter_stage(&mut co, info, 32).await;
                state.add_progress(&mut co, 8, 10).await;
                state.add_progress(&mut co, 8, 20).await;
                let mut state = state.enter_fetch(&mut co).await;
                state.add_progress(&mut co, 16, 30).await;
                let state = state.enter_commit(&mut co).await;
                let state = state.enter_wait_to_reboot(&mut co).await;
                state.enter_reboot(&mut co).await;
            })
            .await,
            vec![
                State::Prepare,
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build()
                    )
                    .unwrap()
                ),
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.25).bytes_downloaded(10).build()
                    )
                    .unwrap()
                ),
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.5).bytes_downloaded(30).build()
                    )
                    .unwrap()
                ),
                State::Fetch(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.5).bytes_downloaded(30).build()
                    )
                    .unwrap()
                ),
                State::Fetch(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(1.0).bytes_downloaded(60).build()
                    )
                    .unwrap()
                ),
                State::Commit(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(1.0).bytes_downloaded(60).build()
                    )
                    .unwrap()
                ),
                State::WaitToReboot(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(1.0).bytes_downloaded(60).build()
                    )
                    .unwrap()
                ),
                State::Reboot(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(1.0).bytes_downloaded(60).build()
                    )
                    .unwrap()
                ),
            ]
        );

        assert_eq!(
            collect_states(|mut co| async move {
                WaitToReboot { info, bytes_downloaded: 60 }.enter_defer_reboot(&mut co).await;
            })
            .await,
            vec![State::DeferReboot(
                UpdateInfoAndProgress::new(
                    info,
                    Progress::builder().fraction_completed(1.0).bytes_downloaded(60).build()
                )
                .unwrap()
            ),]
        );
    }

    #[fuchsia::test]
    async fn yields_expected_states_fail_prepare() {
        assert_eq!(
            collect_states(|mut co| async move {
                let state = Prepare::enter(&mut co).await;
                state.fail(&mut co, PrepareFailureReason::Internal).await
            })
            .await,
            vec![State::Prepare, State::FailPrepare(PrepareFailureReason::Internal),]
        );
    }

    #[fuchsia::test]
    async fn yields_expected_states_fail_fetch() {
        let info = UpdateInfo::builder().download_size(0).build();

        assert_eq!(
            collect_states(|mut co| async move {
                let state = Prepare::enter(&mut co).await;
                let state = state
                    .enter_stage(&mut co, UpdateInfo::builder().download_size(0).build(), 4)
                    .await;
                let mut state = state.enter_fetch(&mut co).await;
                state.add_progress(&mut co, 1, 10).await;
                state.fail(&mut co, FetchFailureReason::Internal).await
            })
            .await,
            vec![
                State::Prepare,
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build()
                    )
                    .unwrap()
                ),
                State::Fetch(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build()
                    )
                    .unwrap()
                ),
                State::Fetch(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.25).bytes_downloaded(10).build()
                    )
                    .unwrap()
                ),
                State::FailFetch(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.25).bytes_downloaded(10).build()
                    )
                    .unwrap()
                    .with_fetch_reason(FetchFailureReason::Internal)
                ),
            ]
        );
    }

    #[fuchsia::test]
    async fn yields_expected_states_fail_stage() {
        let info = UpdateInfo::builder().download_size(0).build();

        assert_eq!(
            collect_states(|mut co| async move {
                let state = Prepare::enter(&mut co).await;
                let mut state = state
                    .enter_stage(&mut co, UpdateInfo::builder().download_size(0).build(), 4)
                    .await;
                state.add_progress(&mut co, 2, 20).await;
                state.add_progress(&mut co, 1, 10).await;
                state.fail(&mut co, StageFailureReason::Internal).await
            })
            .await,
            vec![
                State::Prepare,
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build()
                    )
                    .unwrap()
                ),
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.5).bytes_downloaded(20).build()
                    )
                    .unwrap()
                ),
                State::Stage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.75).bytes_downloaded(30).build()
                    )
                    .unwrap()
                ),
                State::FailStage(
                    UpdateInfoAndProgress::new(
                        info,
                        Progress::builder().fraction_completed(0.75).bytes_downloaded(30).build()
                    )
                    .unwrap()
                    .with_stage_reason(StageFailureReason::Internal)
                ),
            ]
        );
    }

    #[test]
    fn throttling_skips_small_updates_in_short_time() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        let generator = async_generator::generate(|mut co| async move {
            let info = UpdateInfo::builder().download_size(0).build();
            let mut state = Prepare::enter(&mut co).await.enter_stage(&mut co, info, 100_000).await;

            state.add_progress(&mut co, 10, 0).await;
            state.add_progress(&mut co, 10, 0).await;
            state.add_progress(&mut co, 1000, 0).await;
        })
        .into_yielded();
        let mut generator = std::pin::pin!(generator);

        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Prepare))
        );
        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Stage(uip)))
                if uip.progress().fraction_completed() == 0.0
        );
        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Stage(uip)))
                if uip.progress().fraction_completed() == 0.0001
        );
        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Stage(uip)))
                if uip.progress().fraction_completed() == 0.0102
        );
    }

    #[test]
    fn throttling_reports_after_time_elapsed() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (tx, rx) = futures::channel::oneshot::channel();

        let generator = async_generator::generate(|mut co| async move {
            let info = UpdateInfo::builder().download_size(0).build();
            let mut state = Prepare::enter(&mut co).await.enter_stage(&mut co, info, 100_000).await;

            state.add_progress(&mut co, 10, 0).await;
            state.add_progress(&mut co, 10, 0).await;
            let _ = rx.await;
            state.add_progress(&mut co, 10, 0).await;
        })
        .into_yielded();
        let mut generator = std::pin::pin!(generator);

        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Prepare))
        );
        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Stage(uip)))
                if uip.progress().fraction_completed() == 0.0
        );
        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Stage(uip)))
                if uip.progress().fraction_completed() == 0.0001
        );
        assert_matches!(exec.run_until_stalled(&mut generator.next()), Poll::Pending);

        exec.set_fake_time(exec.now() + Duration::from_millis(100).into());
        tx.send(()).unwrap();

        assert_matches!(
            exec.run_until_stalled(&mut generator.next()),
            Poll::Ready(Some(State::Stage(uip)))
                if uip.progress().fraction_completed() == 0.0003
        );
    }
}
