// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude::*;
use fidl::client::QueryResponseFut;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_net_dhcpv6::{
    AcquirePrefixConfig, PrefixControlEvent, PrefixControlExitReason, PrefixControlMarker,
    PrefixControlProxy, PrefixEvent, PrefixProviderMarker,
};
use fuchsia_async::Task;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::Mutex;
use futures::StreamExt as _;
use packet::NoOpSerializationContext;

use super::DEFAULT_TIMEOUT;

use std::net::Ipv6Addr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

const PREFIX_LEN: u8 = 64;

#[derive(Debug, Default, Clone)]
pub struct DhcpV6Pd {
    inner: Arc<Mutex<DhcpV6PdInner>>,
}

#[derive(Debug, Default)]
pub struct DhcpV6PdInner {
    prefix_control: Option<PrefixControlProxy>,
    prefix_watch: Option<QueryResponseFut<PrefixEvent>>,
    prefix: Option<ot::Ip6Prefix>,
    valid: zx::MonotonicInstant,
    preferred: zx::MonotonicInstant,
    last_state: ot::BorderRoutingDhcp6PdState,
    waker: Option<Waker>,
    refresh_task: Option<Task<()>>,
}

fn convert_zx_time_into_seconds_until(time: zx::MonotonicInstant) -> u32 {
    let duration = time - zx::MonotonicInstant::get();

    if duration == zx::MonotonicDuration::INFINITE {
        u32::MAX
    } else if duration <= zx::MonotonicDuration::ZERO {
        u32::MIN
    } else {
        duration.into_seconds().try_into().unwrap_or(u32::MAX)
    }
}

fn make_fake_ra_prefix_packet(prefix: ot::Ip6Prefix, valid: u32, preferred: u32) -> Vec<u8> {
    use net_types::ip::Ipv6;
    use packet::{InnerPacketBuilder, NestablePacketBuilder as _, Serializer};
    use packet_formats::icmp::ndp::options::{NdpOptionBuilder, PrefixInformation};
    use packet_formats::icmp::ndp::{OptionSequenceBuilder, RoutePreference, RouterAdvertisement};
    use packet_formats::icmp::{IcmpPacketBuilder, IcmpZeroCode};

    let src_addr = Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1); // Local host address
    let dst_addr = Ipv6Addr::new(0xFF02, 0, 0, 0, 0, 0, 0, 2); // All routers multicast
    let hop_limit = 100;
    let managed_flag = false;
    let other_config_flag = false;
    let router_lifetime_seconds = u16::MAX;
    let reachable_time_seconds = u32::MAX;
    let retransmit_timer_seconds = u32::MAX;

    // Note: These fields are ignored by OpenThread, so these are placeholders.
    let ra = RouterAdvertisement::with_prf(
        hop_limit,
        managed_flag,
        other_config_flag,
        RoutePreference::default(),
        router_lifetime_seconds,
        reachable_time_seconds,
        retransmit_timer_seconds,
    );

    let prefix_information = PrefixInformation::new(
        prefix.prefix_len(),
        true, // On-Link, Ignored by OpenThread
        true, // Autonomous, Ignored by OpenThread
        valid,
        preferred,
        (*prefix.addr()).into(),
    );

    let options = &[NdpOptionBuilder::PrefixInformation(prefix_information)];

    let serialized = IcmpPacketBuilder::<Ipv6, _>::new(src_addr, dst_addr, IcmpZeroCode, ra)
        .wrap_body(OptionSequenceBuilder::new(options.iter()).into_serializer())
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .unwrap()
        .as_ref()
        .to_vec();

    serialized
}

// Updates OpenThread about the added prefix
fn dhcp_v6_pd_prefix_assigned<T: ot::BorderRouter>(
    instance: &T,
    prefix: ot::Ip6Prefix,
    valid: zx::MonotonicInstant,
    preferred: zx::MonotonicInstant,
) {
    // Here we need to construct a fake ICMPv6 RA
    // that we can feed into OpenThread to let it
    // know about our delegated prefix via DHCPv6-PD.

    let valid = convert_zx_time_into_seconds_until(valid);
    let preferred = convert_zx_time_into_seconds_until(preferred);
    let fake_ra = make_fake_ra_prefix_packet(prefix, valid, preferred);

    instance
        .border_routing_process_icmp6_ra(&fake_ra)
        .expect("Wrong size returned from border_routing_process_icmp6_ra");
}

// Updates OpenThread about the removed prefix
fn dhcp_v6_pd_prefix_unassigned<T: ot::BorderRouter>(instance: &T, prefix: ot::Ip6Prefix) {
    // Here we need to construct a fake ICMPv6 RA
    // that we can feed into OpenThread to let it
    // know that our previously delegated prefix
    // is no longer assigned to us.
    let fake_ra = make_fake_ra_prefix_packet(prefix, 0, 0);

    instance
        .border_routing_process_icmp6_ra(&fake_ra)
        .expect("Wrong size returned from border_routing_process_icmp6_ra");
}

impl DhcpV6PdInner {
    fn abandon_current_prefix(&mut self, instance: &ot::Instance) {
        if let Some(prefix) = self.prefix {
            let prefix_len = prefix.prefix_len();
            let prefix_addr: &std::net::Ipv6Addr = prefix.addr();
            info!(tag = "dhcp_v6_pd"; "Abandoning current prefix {} with length {}", prefix_addr, prefix_len);

            dhcp_v6_pd_prefix_unassigned(instance, prefix);

            self.prefix = None;
        }
    }

    fn update_current_prefix(&mut self, instance: &ot::Instance) {
        if let Some(prefix) = self.prefix {
            dhcp_v6_pd_prefix_assigned(instance, prefix, self.valid, self.preferred);
        }
    }
}

impl DhcpV6Pd {
    pub async fn process_pd_state_change(&self, state: ot::BorderRoutingDhcp6PdState) -> Result {
        {
            let inner = self.inner.lock();
            if state == inner.last_state {
                return Ok(());
            }
        }

        match state {
            ot::BorderRoutingDhcp6PdState::Running => self.start().await?,
            _ => self.stop().await,
        }

        self.inner.lock().last_state = state;
        Ok(())
    }

    pub async fn start(&self) -> Result<(), anyhow::Error> {
        // Ensure stop in case the online task is not terminated.
        self.stop().await;

        info!(tag = "dhcp_v6_pd"; "Starting attempt to lease a prefix via DHCPv6-PD...");
        let prefix_provider =
            connect_to_protocol::<PrefixProviderMarker>().context("dhcpv6pd.start")?;

        let (client, server) = create_endpoints::<PrefixControlMarker>();

        prefix_provider
            .acquire_prefix(
                &AcquirePrefixConfig {
                    preferred_prefix_len: Some(PREFIX_LEN),
                    ..AcquirePrefixConfig::default()
                },
                server,
            )
            .context("dhcpv6pd.start")?;

        let prefix_control = client.into_proxy();
        let watcher = prefix_control.watch_prefix();

        let mut inner = self.inner.lock();
        inner.prefix_control = Some(prefix_control);
        inner.prefix_watch = Some(watcher);

        // Make sure our `poll()` method gets called.
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }

        let inner_clone = self.inner.clone();
        inner.refresh_task = Some(Task::spawn(async move {
            // This loop will make sure that our poll method gets
            // woken up at least once every 15 minutes. This
            // helps make sure that the prefix update is in good shape.

            // Set our refresh duration to 15 minutes.
            const REFRESH_DURATION: fuchsia_async::MonotonicDuration =
                fuchsia_async::MonotonicDuration::from_minutes(15);

            loop {
                // Wait for the refresh duration.
                fuchsia_async::Timer::new(REFRESH_DURATION).await;

                // Lock our inner.
                let mut inner = inner_clone.lock();

                info!(tag = "dhcp_v6_pd"; "Refreshing DHCPv6-PD RA for OpenThread");

                // Make sure our `poll()` method gets called by waking up
                // the waker.
                if let Some(waker) = inner.waker.take() {
                    waker.wake();
                }
            }
        }));

        Ok(())
    }

    pub async fn stop(&self) {
        let prefix_control = {
            let mut inner = self.inner.lock();
            inner.prefix_watch = None;
            inner.refresh_task = None;

            if let Some(prefix_control) = inner.prefix_control.take() {
                info!(tag = "dhcp_v6_pd"; "STOPPING attempt to lease a prefix via DHCPv6-PD.");
                // Make sure our `poll()` method gets called.
                if let Some(waker) = inner.waker.take() {
                    waker.wake();
                }
                Some(prefix_control)
            } else {
                None
            }
        };

        if let Some(prefix_control) = prefix_control {
            if let Err(e) = prefix_control.stop() {
                warn!(tag = "dhcp_v6_pd"; "Failed to send Stop to PrefixControl: {:?}", e);
            }
            let mut event_stream = prefix_control.take_event_stream();

            // Bound the wait. A peer that closes the channel is handled by the
            // `None` arm below, but one that stays alive without ever sending
            // `OnExit` would block us forever. Drop the channel to eventually
            // stop prefix acquisition.
            let exit = event_stream
                .next()
                .on_timeout(fasync::MonotonicInstant::after(DEFAULT_TIMEOUT), || {
                    warn!(tag = "dhcp_v6_pd"; "Timed out waiting for PrefixControl to exit");
                    None
                })
                .await;

            match exit {
                Some(Ok(PrefixControlEvent::OnExit {
                    reason: PrefixControlExitReason::Stopped,
                })) => {
                    info!(tag = "dhcp_v6_pd"; "PrefixControl exited: Stopped");
                }
                Some(Ok(PrefixControlEvent::OnExit { reason })) => {
                    warn!(
                        tag = "dhcp_v6_pd";
                        "PrefixControl exited with unexpected reason: {:?}",
                        reason
                    );
                }
                Some(Err(e)) => {
                    warn!(tag = "dhcp_v6_pd"; "Error waiting for PrefixControl exit: {:?}", e);
                }
                None => {}
            }
        }
    }

    /// Async entrypoint. Called from [`DhcpV6PdPollerExt::dhcp_v6_pd_poll`].
    fn poll(&self, instance: &ot::Instance, cx: &mut Context<'_>) -> std::task::Poll<Result> {
        let mut inner = self.inner.lock();
        inner.waker.replace(cx.waker().clone());

        match (inner.prefix_control.clone(), inner.prefix) {
            (None, None) => {
                // Do nothing in this case.
            }

            (Some(prefix_control), _) => loop {
                // We have a prefix control. This code will
                // loop until the prefix watch no longer returns
                // `Poll::Pending`.
                if inner.prefix_watch.is_some() {
                    match inner.prefix_watch.as_mut().unwrap().poll_unpin(cx) {
                        Poll::Ready(Ok(PrefixEvent::Unassigned(_))) => {
                            info!(tag = "dhcp_v6_pd"; "DHCPv6 Prefix Unassigned");
                            inner.abandon_current_prefix(instance);
                        }
                        Poll::Ready(Ok(PrefixEvent::Assigned(prefix))) => {
                            let prefix_prefix = ot::Ip6Prefix::from(prefix.prefix);

                            if inner.prefix.is_some() && inner.prefix != Some(prefix_prefix) {
                                inner.abandon_current_prefix(instance);
                            }

                            inner.prefix = Some(prefix_prefix);
                            inner.valid =
                                zx::MonotonicInstant::from_nanos(prefix.lifetimes.valid_until);
                            inner.preferred =
                                zx::MonotonicInstant::from_nanos(prefix.lifetimes.preferred_until);

                            info!(
                                tag = "dhcp_v6_pd";
                                "DHCPv6 Prefix Assigned: {:?}, valid: {}s, preferred: {}s",
                                prefix_prefix,
                                convert_zx_time_into_seconds_until(inner.valid),
                                convert_zx_time_into_seconds_until(inner.preferred),
                            );

                            inner.update_current_prefix(instance);
                        }
                        Poll::Ready(Err(fidl_error)) => {
                            error!(
                                tag = "dhcp_v6_pd";
                                "Error watching prefix control: {:?}", fidl_error
                            );

                            // Change our last state to "stopped" so that we can
                            // re-establish our FIDL connections.
                            inner.last_state = ot::BorderRoutingDhcp6PdState::Stopped;

                            return Poll::Ready(Err(fidl_error).context("DhcpV6Pd"));
                        }
                        Poll::Pending => {
                            inner.update_current_prefix(instance);
                            return Poll::Pending;
                        }
                    }
                }

                // Continue to watch.
                inner.prefix_watch = Some(prefix_control.watch_prefix());
            },
            (None, Some(_)) => {
                // We have no control endpoint, but we have a prefix.
                // We need to remove this prefix from the interface.
                inner.abandon_current_prefix(instance);

                // No more watching.
                inner.prefix_watch = None;
            }
        }
        Poll::Pending
    }
}

#[derive(Debug)]
pub struct DhcpV6PdPoller<'a, T: ?Sized>(&'a T);
impl<'a, T: DhcpV6PdPollerExt + ?Sized> Future for DhcpV6PdPoller<'a, T> {
    type Output = Result;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.dhcp_v6_pd_poll(cx)
    }
}

pub trait DhcpV6PdPollerExt {
    fn dhcp_v6_pd_poll(&self, cx: &mut Context<'_>) -> Poll<Result>;

    fn dhcp_v6_pd_future(&self) -> DhcpV6PdPoller<'_, Self> {
        DhcpV6PdPoller(self)
    }
}

impl<T: AsRef<ot::Instance> + AsRef<DhcpV6Pd>> DhcpV6PdPollerExt for fuchsia_sync::Mutex<T> {
    fn dhcp_v6_pd_poll(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result> {
        let guard = self.lock();

        let ot: &ot::Instance = guard.as_ref();
        let dhcp_v6_pd: &DhcpV6Pd = guard.as_ref();
        dhcp_v6_pd.poll(ot, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_net_dhcpv6::PrefixControlRequest;

    #[fuchsia::test]
    async fn test_dhcpv6_pd_stop_when_not_started() {
        let dhcpv6_pd = DhcpV6Pd::default();
        // Calling stop on an unstarted DhcpV6Pd should succeed immediately
        // without error or hang.
        dhcpv6_pd.stop().await;
        assert!(dhcpv6_pd.inner.lock().prefix_control.is_none());
    }

    #[fuchsia::test]
    async fn test_dhcpv6_pd_stop_waits_for_on_exit() {
        let (prefix_control, mut stream) = create_proxy_and_stream::<PrefixControlMarker>();
        let dhcpv6_pd = DhcpV6Pd::default();
        dhcpv6_pd.inner.lock().prefix_control = Some(prefix_control);

        let mut stop_fut = std::pin::pin!(dhcpv6_pd.stop());

        // `Stop` is sent on the first poll, after which `stop()` must remain
        // pending until the server acknowledges.
        assert!(futures::poll!(stop_fut.as_mut()).is_pending());

        let control_handle = assert_matches!(
            stream.next().await,
            Some(Ok(PrefixControlRequest::Stop { control_handle })) => control_handle
        );
        assert!(futures::poll!(stop_fut.as_mut()).is_pending());

        control_handle.send_on_exit(PrefixControlExitReason::Stopped).expect("send OnExit");
        stop_fut.await;

        let inner = dhcpv6_pd.inner.lock();
        assert!(inner.prefix_control.is_none());
        assert!(inner.prefix_watch.is_none());
    }

    #[fuchsia::test]
    async fn test_dhcpv6_pd_process_state_change_stopped_calls_stop() {
        let (prefix_control, mut stream) = create_proxy_and_stream::<PrefixControlMarker>();
        let dhcpv6_pd = DhcpV6Pd::default();
        {
            let mut inner = dhcpv6_pd.inner.lock();
            inner.last_state = ot::BorderRoutingDhcp6PdState::Running;
            inner.prefix_control = Some(prefix_control);
        }

        let mut state_change_fut = std::pin::pin!(
            dhcpv6_pd.process_pd_state_change(ot::BorderRoutingDhcp6PdState::Stopped)
        );
        assert!(futures::poll!(state_change_fut.as_mut()).is_pending());

        let control_handle = assert_matches!(
            stream.next().await,
            Some(Ok(PrefixControlRequest::Stop { control_handle })) => control_handle
        );
        control_handle.send_on_exit(PrefixControlExitReason::Stopped).expect("send OnExit");

        assert_matches!(state_change_fut.await, Ok(()));

        let inner = dhcpv6_pd.inner.lock();
        assert_eq!(inner.last_state, ot::BorderRoutingDhcp6PdState::Stopped);
        assert!(inner.prefix_control.is_none());
    }

    #[fuchsia::test]
    async fn test_dhcpv6_pd_stop_handles_unexpected_exit_reason() {
        let (prefix_control, mut stream) = create_proxy_and_stream::<PrefixControlMarker>();
        let dhcpv6_pd = DhcpV6Pd::default();
        dhcpv6_pd.inner.lock().prefix_control = Some(prefix_control);

        let mut stop_fut = std::pin::pin!(dhcpv6_pd.stop());
        assert!(futures::poll!(stop_fut.as_mut()).is_pending());

        let control_handle = assert_matches!(
            stream.next().await,
            Some(Ok(PrefixControlRequest::Stop { control_handle })) => control_handle
        );

        // An unexpected reason is logged rather than treated as an error, but
        // it must still unblock `stop()`.
        control_handle
            .send_on_exit(PrefixControlExitReason::InterfaceRemoved)
            .expect("send OnExit");
        stop_fut.await;

        let inner = dhcpv6_pd.inner.lock();
        assert!(inner.prefix_control.is_none());
        assert!(inner.prefix_watch.is_none());
    }

    // Uses fake time so the test does not have to wait out the timeout.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_dhcpv6_pd_stop_times_out_when_server_never_exits() {
        // Hold the request stream without servicing it, mimicking a peer that
        // is alive but wedged rather than one that closes the channel.
        let (prefix_control, _stream) = create_proxy_and_stream::<PrefixControlMarker>();
        let dhcpv6_pd = DhcpV6Pd::default();
        dhcpv6_pd.inner.lock().prefix_control = Some(prefix_control);

        let mut stop_fut = std::pin::pin!(dhcpv6_pd.stop());
        assert!(futures::poll!(stop_fut.as_mut()).is_pending());

        fuchsia_async::TestExecutor::advance_to(fuchsia_async::MonotonicInstant::after(
            DEFAULT_TIMEOUT,
        ))
        .await;

        assert!(futures::poll!(stop_fut.as_mut()).is_ready());

        let inner = dhcpv6_pd.inner.lock();
        assert!(inner.prefix_control.is_none());
        assert!(inner.prefix_watch.is_none());
    }
}
