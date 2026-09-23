// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 bindings system power handling.

use fuchsia_async as fasync;
use futures::{FutureExt as _, StreamExt};
use log::{debug, warn};
use netstack3_core::device::WeakDeviceId;

use crate::bindings::devices::{DeviceSpecificInfo, EthernetInfo, PureIpDeviceInfo};
use crate::bindings::power::element::InternalPowerElement;
use crate::bindings::power::suspension_block::MaybeSuspensionGuard;
use crate::bindings::power::{BinaryPowerLevel, LessorRegistration};
use crate::bindings::{BindingsCtx, Ctx, DeviceIdExt};

const SUSPENSION_WARN_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(10);

/// Handles tx path suspension for devices.
pub(crate) struct TransmitSuspensionHandler {
    power_element: InternalPowerElement,
    suspension_guard: MaybeSuspensionGuard,
    device: WeakDeviceId<BindingsCtx>,
    _lessor_registration: LessorRegistration,
}

impl TransmitSuspensionHandler {
    /// Creates a new `TransmitSuspensionHandler` that communicates with the
    /// `PowerWorker` in `ctx`.
    pub(crate) async fn new(ctx: &Ctx, device: WeakDeviceId<BindingsCtx>) -> Self {
        let power_element = InternalPowerElement::new();
        let registration =
            ctx.bindings_ctx().power.register_internal_lessor(power_element.lessor());

        let mut this = Self {
            power_element,
            suspension_guard: MaybeSuspensionGuard::new(
                ctx.bindings_ctx().power.suspension_block.clone(),
            ),
            device,
            _lessor_registration: registration,
        };

        // Power elements start in off state. Wait until our lease registration
        // has told us to be active before returning from `new`.
        this.wait_for_level_on().await;

        this
    }

    /// Waits for a suspension request.
    pub(crate) async fn wait(&mut self) -> TransmitSuspensionRequest<'_> {
        let Self { power_element, suspension_guard: _, device: _, _lessor_registration } = self;
        // May only be called if the power element is on.
        assert_eq!(power_element.current_level(), BinaryPowerLevel::On);
        assert_eq!(power_element.wait_level_change().await, BinaryPowerLevel::Off);
        TransmitSuspensionRequest(self)
    }

    async fn wait_for_level_on(&mut self) {
        let Self { power_element, suspension_guard, device: _, _lessor_registration } = self;
        loop {
            assert_eq!(power_element.wait_level_change().await, BinaryPowerLevel::On);

            let new_level = futures::select! {
                nl = power_element.wait_level_change().fuse() => nl,
                acquired = suspension_guard.acquire().fuse() => {
                    assert!(acquired);
                    break;
                }
            };
            // Power level changed back to off while we were waiting for a
            // suspension guard, loop and wait for it to be on again.
            assert_eq!(new_level, BinaryPowerLevel::Off);
        }
    }
}

pub(crate) struct TransmitSuspensionRequest<'a>(&'a mut TransmitSuspensionHandler);

impl TransmitSuspensionRequest<'_> {
    /// Handles a suspension request, blocking until the system resumes.
    ///
    /// Waits for the transmitting device to finish its work and then signals
    /// back to power framework that our power element's level is lowered. Then
    /// it waits for the level of our power element to rise before returning.
    /// The rise in power level indicates that it is okay for us to resume
    /// normal operation.
    pub(crate) async fn handle_suspension(self) {
        let Self(handler) = self;
        let TransmitSuspensionHandler {
            power_element,
            suspension_guard,
            device,
            _lessor_registration,
        } = handler;
        assert_eq!(power_element.current_level(), BinaryPowerLevel::Off);
        let name = device.bindings_id();

        // Wait for the device to be ready for suspension. Meanwhile, PF is
        // waiting for us to notify it that we've transitioned to the disabled
        // state.
        debug!("handling suspension for {name}. Waiting for tx done.");
        let warn_interval = fasync::Interval::new(SUSPENSION_WARN_INTERVAL).map(|()| {
            warn!("device {name} still pending tx suspension");
        });
        futures::select! {
            () = wait_device_tx_suspension_ready(device).fuse() => (),
            () = warn_interval.collect::<()>() => unreachable!(),
        }
        debug!("suspension handling for {name} complete.");

        // Drop our guard on suspension.
        assert!(suspension_guard.release());
        // Wait for the power level to go back to on.
        handler.wait_for_level_on().await;

        debug!("resumed device {} from suspension", handler.device.bindings_id());
    }
}

async fn wait_device_tx_suspension_ready(device: &WeakDeviceId<BindingsCtx>) {
    let Some(device) = device.upgrade() else {
        return;
    };
    match device.external_state() {
        DeviceSpecificInfo::Loopback(_) => {
            // No suspension handling for loopback.
        }
        DeviceSpecificInfo::Blackhole(_) => {
            // No suspension handling for blackhole devices.
        }
        DeviceSpecificInfo::Ethernet(EthernetInfo { netdevice, .. })
        | DeviceSpecificInfo::PureIp(PureIpDeviceInfo { netdevice, .. }) => {
            // For netdevice backed devices, wait for all tx descriptors to be
            // done before continuing with suspension.
            netdevice.handler.wait_tx_done().await;
        }
    };
}
