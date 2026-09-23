// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::time::Duration;

use fidl_fuchsia_net_http as fnet_http;
use futures::{Stream, StreamExt};
use log::{debug, info, warn};
use replace_with::replace_with;

use crate::NetstackVersion;

/// The time to wait between healthchecks.
const HEALTHCHECK_INTERVAL: Duration = Duration::from_secs(5 * 60 * 60);

/// The duration after boot at which to start the first healthcheck.
const HEALTHCHECK_STARTUP_DELAY: Duration = Duration::from_secs(5 * 60);

/// The number of failed healthchecks before we decide Netstack3 isn't working
/// and should roll back to Netstack2.
pub(crate) const MAX_FAILED_HEALTHCHECKS: usize = 5;

// The URL and method are chosen to match the Cast connectivity checking behavior.
//
// See the following in chromium:
//
//   - chromecast/net/connectivity_checker_impl.{h,cc}
const HEALTHCHECK_URL: &str = "https://connectivitycheck.gstatic.com/generate_204";

/// The in-memory state machine for the Netstack3 rollback system.
///
/// All update methods return a modified [`State`] for easier testing.
///
/// Communication with other systems (e.g. performing healthchecks and
/// scheduling reboots) must be handled by a higher-level system.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// We are currently running Netstack2, so no functionality should be
    /// enabled for the current boot.
    ///
    /// This state is entered at boot and will never be left.
    Netstack2,

    /// We are running Netstack3 and should check whether connectivity is live.
    ///
    /// If the number of failed checks reaches [`MAX_FAILED_HEALTHCHECKS`], a
    /// reboot is scheduled and the next boot will be forced to use Netstack2 in
    /// order to regain connectivity.
    ///
    /// State transitions:
    ///
    /// - On a successful healthcheck, transition to Success.
    /// - On a failed healthchek, add one to the number of failed checks, and
    ///   re-enter Checking.
    /// - When the desired netstack becomes Netstack2, enter Canceled.
    Checking(usize),

    /// The migration was cancelled while we were already running Netstack3.
    ///
    /// State transitions depend on the inner value. This is to handle the case
    /// where the desired netstack version returns to Netstack3 after the
    /// migration is canceled.
    ///
    /// - When the desired netstack becomes Netstack3:
    ///   - If [`Canceled::FromChecking`], return to Checking with the contained value.
    ///   - If [`Canceled::FromSuccess`], return to Success.
    Canceled(Canceled),

    /// Netstack3 healthchecked successfully and so we assume we can safely
    /// continue running Netstack3.
    ///
    /// State transitions:
    ///
    /// - When the desired netstack becomes Netstack2, enter Canceled.
    Success,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Canceled {
    FromChecking(usize),
    FromSuccess,
}

impl State {
    /// Create a new state based on the persisted rollback state as well as the
    /// currently-running Netstack version.
    pub(crate) fn new(persisted: Option<Persisted>, current_boot: NetstackVersion) -> Self {
        match (persisted, current_boot) {
            (_, NetstackVersion::Netstack2) => State::Netstack2,
            (None, NetstackVersion::Netstack3) => State::Checking(0),
            (Some(Persisted::HealthcheckFailures(failures)), NetstackVersion::Netstack3) => {
                // We increment the number of failures in case Netstack3 is
                // crashing, which reboots the device.
                State::Checking(failures + 1)
            }
            (Some(Persisted::Success), NetstackVersion::Netstack3) => State::Success,
        }
    }

    /// Called when a new desired netstack version is selected. Returns the new
    /// [`State`].
    fn on_desired_version_change(self, desired_version: NetstackVersion) -> Self {
        let old = self.clone();
        let new = match (self, desired_version) {
            (State::Netstack2, _) => self,

            (State::Checking(failures), NetstackVersion::Netstack2) => {
                State::Canceled(Canceled::FromChecking(failures))
            }
            (State::Checking(_), NetstackVersion::Netstack3) => self,

            (State::Success, NetstackVersion::Netstack2) => State::Canceled(Canceled::FromSuccess),
            (State::Success, NetstackVersion::Netstack3) => self,

            (State::Canceled(_), NetstackVersion::Netstack2) => self,
            (State::Canceled(inner), NetstackVersion::Netstack3) => match inner {
                Canceled::FromChecking(failures) => State::Checking(failures),
                Canceled::FromSuccess => State::Success,
            },
        };

        if new != old {
            info!("on_desired_version_change: Rollback state changed from {old:?} to {new:?}");
        }
        new
    }

    /// Called after a healthcheck. Returns the new [`State`].
    fn on_healthcheck(self, result: HealthcheckResult) -> Self {
        let old = self.clone();
        let new = match self {
            // None of these should be reachable in practice.
            State::Netstack2 | State::Success | State::Canceled(_) => self,

            State::Checking(failures) => match result {
                HealthcheckResult::Success => State::Success,
                HealthcheckResult::Failure => State::Checking(failures + 1),
            },
        };

        if new != old {
            info!("on_healthcheck: Rollback state changed from {old:?} to {new:?}");
        }
        new
    }

    fn should_healthcheck(&self) -> bool {
        match self {
            State::Checking(_) => true,
            State::Netstack2 | State::Success | State::Canceled(_) => false,
        }
    }

    /// Transforms the in-memory state into what should be persisted to disk.
    pub(crate) fn persisted(&self) -> Persisted {
        match self {
            State::Netstack2 => Persisted::HealthcheckFailures(0),
            State::Checking(failures) => Persisted::HealthcheckFailures(*failures),

            State::Success => Persisted::Success,
            State::Canceled(_) => Persisted::HealthcheckFailures(0),
        }
    }
}

/// A very simplified version of the in-memory state that's persisted to disk.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Persisted {
    /// We have attempted to check connectivity while running Netstack3 this
    /// many times without falling back to Netstack2.
    HealthcheckFailures(usize),

    /// We successfully healthchecked against Netstack3 and will no longer
    /// perform a rollback.
    Success,
}

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
enum HealthcheckResult {
    Success,
    Failure,
}

pub(crate) trait HttpFetcher {
    fn fetch(
        &mut self,
        request: fnet_http::Request,
    ) -> impl std::future::Future<Output = fidl::Result<fnet_http::Response>> + Send;
}

pub(crate) struct FidlHttpFetcher(fnet_http::LoaderProxy);

impl FidlHttpFetcher {
    pub(crate) fn new() -> Self {
        let loader = fuchsia_component::client::connect_to_protocol::<fnet_http::LoaderMarker>()
            .expect("unable to connect to fuchsia.net.http.Loader");
        FidlHttpFetcher(loader)
    }
}

impl HttpFetcher for FidlHttpFetcher {
    async fn fetch(&mut self, request: fnet_http::Request) -> fidl::Result<fnet_http::Response> {
        self.0.fetch(request).await
    }
}

struct HttpHealthchecker<R> {
    requester: R,
}

impl<R> HttpHealthchecker<R>
where
    R: HttpFetcher,
{
    async fn healthcheck(&mut self) -> HealthcheckResult {
        let request = fnet_http::Request {
            url: Some(HEALTHCHECK_URL.to_owned()),
            method: Some("HEAD".into()),
            headers: None,
            body: None,
            deadline: None,
            ..Default::default()
        };

        let resp = match self.requester.fetch(request).await {
            Ok(r) => r,
            Err(e) => {
                warn!("FIDL error while sending HTTP request: {e:?}");
                return HealthcheckResult::Failure;
            }
        };

        // There was a network-level error.
        if let Some(err) = resp.error {
            warn!("network error while sending HTTP request: {err:?}");
            return HealthcheckResult::Failure;
        }

        match resp.status_code {
            Some(code) => {
                if code == 204 {
                    info!("HTTP healthcheck successful");
                    HealthcheckResult::Success
                } else if code >= 200 && code < 300 {
                    warn!("unexpectedly received non-204 success: {code}");
                    HealthcheckResult::Failure
                } else {
                    warn!("received non-success status: {code}");
                    HealthcheckResult::Failure
                }
            }

            // Because we already checked for a network error, this shouldn't be
            // reached in practice.
            None => {
                warn!("no status code found");
                HealthcheckResult::Failure
            }
        }
    }
}

pub(crate) fn new_healthcheck_stream() -> impl futures::stream::Stream<Item = ()> {
    futures::stream::once(fuchsia_async::Timer::new(HEALTHCHECK_STARTUP_DELAY))
        .chain(fuchsia_async::Interval::new(HEALTHCHECK_INTERVAL.into()))
}

/// Implements the full Netstack3 rollback lifecycle.
///
/// Scheduling and canceling reboots is delegated to the main stack migration
/// code, which has a wider view of the world.
pub(crate) async fn run<H, T>(
    mut state: State,
    http_fetcher: H,
    desired_version_updates: futures::channel::mpsc::UnboundedReceiver<NetstackVersion>,
    persistance_updates: futures::channel::mpsc::UnboundedSender<Persisted>,
    healthcheck_tick: T,
) where
    H: HttpFetcher,
    T: Stream<Item = ()> + Unpin,
{
    let mut health_checker = HttpHealthchecker { requester: http_fetcher };
    enum Action {
        Healthcheck,
        NewDesiredVersion(NetstackVersion),
    }

    let mut stream = futures::stream::select(
        healthcheck_tick.map(|()| Action::Healthcheck),
        desired_version_updates.map(Action::NewDesiredVersion),
    );

    while let Some(action) = stream.next().await {
        match action {
            Action::Healthcheck => {
                if state.should_healthcheck() {
                    info!("running healthcheck");
                    let hc_result = health_checker.healthcheck().await;
                    replace_with(&mut state, |state| state.on_healthcheck(hc_result));
                }
            }
            Action::NewDesiredVersion(version) => {
                debug!("new desired netstack version: {version:?}");
                replace_with(&mut state, |state| state.on_desired_version_change(version));
            }
        }

        persistance_updates.unbounded_send(state.persisted()).unwrap()
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub(crate) struct MockHttpRequester<F>(pub(crate) F);

    impl<F> HttpFetcher for MockHttpRequester<F>
    where
        F: FnMut() -> fidl::Result<fidl_fuchsia_net_http::Response>,
    {
        fn fetch(
            &mut self,
            _request: fnet_http::Request,
        ) -> impl futures::future::Future<Output = fidl::Result<fidl_fuchsia_net_http::Response>> + Send
        {
            futures::future::ready(self.0())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use assert_matches::assert_matches;
    use fidl_fuchsia_net_http as fnet_http;
    use fuchsia_async::Task;
    use futures::SinkExt;
    use futures::channel::mpsc;
    use test_case::test_case;

    use crate::NetstackVersion;
    use crate::rollback::testutil::MockHttpRequester;

    #[test_case(None, NetstackVersion::Netstack2 => State::Netstack2)]
    #[test_case(None, NetstackVersion::Netstack3 => State::Checking(0))]
    #[test_case(
        Some(Persisted::HealthcheckFailures(10)),
        NetstackVersion::Netstack2 => State::Netstack2
    )]
    #[test_case(
        Some(Persisted::HealthcheckFailures(10)),
        NetstackVersion::Netstack3 => State::Checking(11)
    )]
    #[test_case(Some(Persisted::Success), NetstackVersion::Netstack2 => State::Netstack2)]
    #[test_case(Some(Persisted::Success), NetstackVersion::Netstack3 => State::Success)]
    fn test_state_construction(
        persisted: Option<Persisted>,
        current_boot: NetstackVersion,
    ) -> State {
        State::new(persisted, current_boot)
    }

    #[test_case(State::Netstack2, NetstackVersion::Netstack2 => State::Netstack2)]
    #[test_case(State::Netstack2, NetstackVersion::Netstack3 => State::Netstack2)]
    #[test_case(
        State::Checking(1),
        NetstackVersion::Netstack2 => State::Canceled(Canceled::FromChecking(1)))]
    #[test_case(
        State::Checking(1),
        NetstackVersion::Netstack3 => State::Checking(1))]
    #[test_case(
        State::Checking(MAX_FAILED_HEALTHCHECKS+1),
        NetstackVersion::Netstack2 =>
            State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1))
    )]
    #[test_case(
        State::Checking(MAX_FAILED_HEALTHCHECKS+1),
        NetstackVersion::Netstack3 => State::Checking(MAX_FAILED_HEALTHCHECKS+1)
    )]
    #[test_case(
        State::Canceled(Canceled::FromChecking(1)),
        NetstackVersion::Netstack2 => State::Canceled(Canceled::FromChecking(1)))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(1)),
        NetstackVersion::Netstack3 => State::Checking(1))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)),
        NetstackVersion::Netstack2 =>
            State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1))
    )]
    #[test_case(
        State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)),
        NetstackVersion::Netstack3 => State::Checking(MAX_FAILED_HEALTHCHECKS+1)
    )]
    #[test_case(
        State::Canceled(Canceled::FromSuccess),
        NetstackVersion::Netstack2 => State::Canceled(Canceled::FromSuccess))]
    #[test_case(
        State::Canceled(Canceled::FromSuccess),
        NetstackVersion::Netstack3 => State::Success)]
    #[test_case(
        State::Success,
        NetstackVersion::Netstack2 => State::Canceled(Canceled::FromSuccess))]
    #[test_case(State::Success, NetstackVersion::Netstack3 => State::Success)]
    fn test_on_desired_version_change(state: State, desired_version: NetstackVersion) -> State {
        state.on_desired_version_change(desired_version)
    }

    #[test_case(State::Netstack2, HealthcheckResult::Success => State::Netstack2)]
    #[test_case(State::Netstack2, HealthcheckResult::Failure => State::Netstack2)]
    #[test_case(State::Checking(1), HealthcheckResult::Success => State::Success)]
    #[test_case(State::Checking(1), HealthcheckResult::Failure => State::Checking(2))]
    #[test_case(
        State::Checking(MAX_FAILED_HEALTHCHECKS+1),
        HealthcheckResult::Success => State::Success)]
    #[test_case(
        State::Checking(MAX_FAILED_HEALTHCHECKS+1),
        HealthcheckResult::Failure => State::Checking(MAX_FAILED_HEALTHCHECKS+2))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(1)),
        HealthcheckResult::Success => State::Canceled(Canceled::FromChecking(1)))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(1)),
        HealthcheckResult::Failure => State::Canceled(Canceled::FromChecking(1)))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)),
        HealthcheckResult::Success =>
            State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)),
        HealthcheckResult::Failure =>
            State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)))]
    #[test_case(
        State::Canceled(Canceled::FromSuccess),
        HealthcheckResult::Success => State::Canceled(Canceled::FromSuccess))]
    #[test_case(
        State::Canceled(Canceled::FromSuccess),
        HealthcheckResult::Failure => State::Canceled(Canceled::FromSuccess))]
    #[test_case(State::Success, HealthcheckResult::Success => State::Success)]
    #[test_case(State::Success, HealthcheckResult::Failure => State::Success)]
    fn test_on_healthcheck(state: State, helthcheck_result: HealthcheckResult) -> State {
        state.on_healthcheck(helthcheck_result)
    }

    #[test_case(State::Netstack2 => false)]
    #[test_case(State::Checking(1) => true)]
    #[test_case(State::Checking(MAX_FAILED_HEALTHCHECKS+1) => true)]
    #[test_case(State::Canceled(Canceled::FromChecking(1)) => false)]
    #[test_case(State::Canceled(Canceled::FromSuccess) => false)]
    #[test_case(State::Success => false)]
    fn test_should_healthcheck(state: State) -> bool {
        state.should_healthcheck()
    }

    #[test_case(
        State::Netstack2 =>
            Persisted::HealthcheckFailures(0))]
    #[test_case(
        State::Checking(1) =>
            Persisted::HealthcheckFailures(1))]
    #[test_case(
        State::Checking(MAX_FAILED_HEALTHCHECKS+1) =>
            Persisted::HealthcheckFailures(MAX_FAILED_HEALTHCHECKS+1))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(1)) =>
            Persisted::HealthcheckFailures(0))]
    #[test_case(
        State::Canceled(Canceled::FromChecking(MAX_FAILED_HEALTHCHECKS+1)) =>
            Persisted::HealthcheckFailures(0))]
    #[test_case(
        State::Canceled(Canceled::FromSuccess) =>
            Persisted::HealthcheckFailures(0))]
    #[test_case(
        State::Success =>
            Persisted::Success)]
    fn test_persisted(state: State) -> Persisted {
        state.persisted()
    }

    #[test_case(
        || {
            Ok(fnet_http::Response{error: None, status_code: Some(204), ..Default::default()})
        } => HealthcheckResult::Success;
        "success"
    )]
    #[test_case(
        || {
            Err(fidl::Error::Invalid)
        } => HealthcheckResult::Failure;
        "failure fidl error")]
    #[test_case(
        || {
            Ok(fnet_http::Response{error: Some(fnet_http::Error::Internal), ..Default::default()})
        } => HealthcheckResult::Failure;
        "failure http error")]
    #[test_case(
        || {
            Ok(fnet_http::Response{error: None, status_code: Some(200), ..Default::default()})
        } => HealthcheckResult::Failure;
        "failure 200")]
    #[test_case(
        || {
            Ok(fnet_http::Response{error: None, status_code: Some(404), ..Default::default()})
        } => HealthcheckResult::Failure;
        "failure 404")]
    #[test_case(
        || {
            Ok(fnet_http::Response{error: None, status_code: None, ..Default::default()})
        } => HealthcheckResult::Failure;
        "failure no status")]
    #[fuchsia::test]
    async fn test_healthchecker(
        response: impl FnMut() -> fidl::Result<fnet_http::Response>,
    ) -> HealthcheckResult {
        let r = MockHttpRequester(response);
        HttpHealthchecker { requester: r }.healthcheck().await
    }

    #[fuchsia::test]
    async fn test_healthcheck_fails_then_succeeds() {
        let mut n = 0;
        let r = MockHttpRequester(move || {
            n += 1;
            if n <= MAX_FAILED_HEALTHCHECKS {
                Ok(fnet_http::Response {
                    error: None,
                    status_code: Some(500),
                    ..Default::default()
                })
            } else {
                Ok(fnet_http::Response {
                    error: None,
                    status_code: Some(204),
                    ..Default::default()
                })
            }
        });

        let state = State::Checking(0);
        let (mut healthcheck_timer_sender, healthcheck_timer_receiver) = mpsc::unbounded();
        let (mut desired_version_sender, desired_version_receiver) = mpsc::unbounded();
        let (persistence_sender, mut persistence_receiver) = mpsc::unbounded();

        let task = Task::spawn(super::run(
            state,
            r,
            desired_version_receiver,
            persistence_sender,
            healthcheck_timer_receiver,
        ));

        for i in 1..=MAX_FAILED_HEALTHCHECKS - 1 {
            healthcheck_timer_sender.send(()).await.unwrap();
            let n = assert_matches!(
                persistence_receiver.next().await.unwrap(),
                Persisted::HealthcheckFailures(n) => n
            );
            assert_eq!(n, i);
        }
        healthcheck_timer_sender.send(()).await.unwrap();
        let n = assert_matches!(
            persistence_receiver.next().await.unwrap(),
            Persisted::HealthcheckFailures(n) => n
        );
        assert_eq!(n, MAX_FAILED_HEALTHCHECKS);

        desired_version_sender.send(NetstackVersion::Netstack2).await.unwrap();
        assert_matches!(
            persistence_receiver.next().await.unwrap(),
            Persisted::HealthcheckFailures(0)
        );

        // This time, the healthcheck will return success, but we're no longer
        // healthchecking because the migration was canceled.
        healthcheck_timer_sender.send(()).await.unwrap();
        assert_matches!(
            persistence_receiver.next().await.unwrap(),
            Persisted::HealthcheckFailures(0)
        );

        desired_version_sender.send(NetstackVersion::Netstack3).await.unwrap();
        let n = assert_matches!(
            persistence_receiver.next().await.unwrap(),
            Persisted::HealthcheckFailures(n) => n
        );
        assert_eq!(n, MAX_FAILED_HEALTHCHECKS);

        healthcheck_timer_sender.send(()).await.unwrap();
        assert_matches!(persistence_receiver.next().await.unwrap(), Persisted::Success);

        // Dropping these two should cause the task to complete.
        drop(healthcheck_timer_sender);
        drop(desired_version_sender);
        task.await;
    }
}
