// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;

use lowpan_driver_common::lowpan_fidl::*;
use std::num::NonZeroU64;

#[derive(Debug)]
pub struct DriverState<OT> {
    pub ot_instance: OT,

    pub connectivity_state: ConnectivityState,

    pub srp_discovery_proxy: Option<DiscoveryProxy>,

    pub srp_advertising_proxy: Option<AdvertisingProxy>,

    pub ot_ctl: ot_ctl::OtCtl,

    pub detailed_logging: detailed_logging::DetailedLogging,

    pub nat64: nat64::Nat64,

    pub dhcp_v6_pd: dhcpv6pd::DhcpV6Pd,

    pub border_agent: border_agent::BorderAgent,

    pub epskc: border_agent::Epskc,

    pub multicast_routing_manager: Option<multicast_routing_manager::MulticastRoutingManager>,
}

impl<OT: AsRef<ot::Instance>> AsRef<ot::Instance> for DriverState<OT> {
    fn as_ref(&self) -> &ot::Instance {
        self.ot_instance.as_ref()
    }
}

impl<OT> AsRef<Option<DiscoveryProxy>> for DriverState<OT> {
    fn as_ref(&self) -> &Option<DiscoveryProxy> {
        &self.srp_discovery_proxy
    }
}

impl<OT> AsRef<Option<AdvertisingProxy>> for DriverState<OT> {
    fn as_ref(&self) -> &Option<AdvertisingProxy> {
        &self.srp_advertising_proxy
    }
}

impl<OT> AsRef<dhcpv6pd::DhcpV6Pd> for DriverState<OT> {
    fn as_ref(&self) -> &dhcpv6pd::DhcpV6Pd {
        &self.dhcp_v6_pd
    }
}

impl<OT> AsRef<border_agent::BorderAgent> for DriverState<OT> {
    fn as_ref(&self) -> &border_agent::BorderAgent {
        &self.border_agent
    }
}

impl<OT> AsRef<border_agent::Epskc> for DriverState<OT> {
    fn as_ref(&self) -> &border_agent::Epskc {
        &self.epskc
    }
}

impl<OT> AsRef<Option<multicast_routing_manager::MulticastRoutingManager>> for DriverState<OT> {
    fn as_ref(&self) -> &Option<multicast_routing_manager::MulticastRoutingManager> {
        &self.multicast_routing_manager
    }
}

impl<OT: openthread::ot::Trel> DriverState<OT> {
    // SAFETY: In general this is safe to call because `bool`s are
    //         safe to write to or read from multiple threads.
    //         This code happens to always be called from a single
    //         thread anyway, so it is going to be safe regardless.
    unsafe fn trel_enabled_ref() -> &'static mut bool {
        static mut TREL_ENABLED: bool = false;
        #[allow(static_mut_refs)]
        unsafe {
            &mut TREL_ENABLED
        }
    }

    pub fn is_trel_enabled(&self) -> bool {
        // SAFETY: This is safe for the reasons explained
        //         in the comment above `trel_enable_ref()`.
        unsafe { *Self::trel_enabled_ref() }
    }

    pub fn set_trel_enabled(&mut self, enabled: bool) {
        if enabled != self.ot_instance.trel_is_enabled() {
            self.ot_instance.trel_set_enabled(enabled);
        }

        // SAFETY: This is safe for the reasons explained
        //         in the comment above `trel_enable_ref()`.
        unsafe {
            *Self::trel_enabled_ref() = enabled;
        }
    }

    pub fn check_trel(&self) {
        if !self.is_trel_enabled() && self.ot_instance.trel_is_enabled() {
            self.ot_instance.trel_set_enabled(false);
        }
    }
}

impl<OT: openthread::ot::BorderRouter> DriverState<OT> {
    // SAFETY: In general this is safe to call because `bool`s are
    //         safe to write to or read from multiple threads.
    //         This code happens to always be called from a single
    //         thread anyway, so it is going to be safe regardless.
    unsafe fn dhcpv6_pd_enabled_ref() -> &'static mut bool {
        static mut DHCPV6_PD_ENABLED: bool = false;
        #[allow(static_mut_refs)]
        unsafe {
            &mut DHCPV6_PD_ENABLED
        }
    }

    pub fn is_dhcpv6_pd_enabled(&self) -> bool {
        // SAFETY: This is safe for the reasons explained
        //         in the comment above `dhcpv6_pd_enable_ref()`.
        unsafe { *Self::dhcpv6_pd_enabled_ref() }
    }

    pub fn set_dhcpv6_pd_enabled(&mut self, enabled: bool) {
        // SAFETY: This is safe for the reasons explained
        //         in the comment above `dhcpv6_pd_enable_ref()`.
        unsafe {
            *Self::dhcpv6_pd_enabled_ref() = enabled;
        }

        info!(
            "DHCPv6-PD has been {}.",
            if self.is_dhcpv6_pd_enabled() { "ENABLED" } else { "DISABLED" }
        );

        self.check_dhcpv6_pd();
    }

    pub fn check_dhcpv6_pd(&self) {
        self.ot_instance.border_routing_dhcp6_pd_set_enabled(self.is_dhcpv6_pd_enabled());
    }
}

impl<OT: openthread::ot::BorderAgent> DriverState<OT> {
    // SAFETY: In general this is safe to call because `bool`s are
    //         safe to write to or read from multiple threads.
    //         This code happens to always be called from a single
    //         thread anyway, so it is going to be safe regardless.
    unsafe fn epskc_enabled_ref() -> &'static mut bool {
        static mut EPSKC_ENABLED: bool = false;
        #[allow(static_mut_refs)]
        unsafe {
            &mut EPSKC_ENABLED
        }
    }

    pub fn is_epskc_enabled(&self) -> bool {
        // SAFETY: This is safe for the reasons explained
        //         in the comment above `epskc_enabled_ref()`.
        unsafe { *Self::epskc_enabled_ref() }
    }

    pub fn set_epskc_enabled(&mut self, enabled: bool) {
        // SAFETY: This is safe for the reasons explained
        //         in the comment above `epskc_enabled_ref()`.
        unsafe {
            *Self::epskc_enabled_ref() = enabled;
        }

        info!("ePSKc has been {}.", if self.is_epskc_enabled() { "ENABLED" } else { "DISABLED" });

        self.epskc_set_enabled_in_ot();
    }

    pub fn epskc_set_enabled_in_ot(&self) {
        self.ot_instance.border_agent_ephemeral_key_set_enabled(self.is_epskc_enabled());
    }
}

impl<OT: AsRef<ot::Instance>> DriverState<OT> {
    pub fn is_discovery_proxy_enabled(&self) -> bool {
        self.srp_discovery_proxy.is_some()
    }

    pub fn set_discovery_proxy_enabled(&mut self, enabled: bool) -> Result {
        if self.is_discovery_proxy_enabled() != enabled {
            let _ = self.srp_discovery_proxy.take();
            if enabled {
                self.srp_discovery_proxy =
                    Some(DiscoveryProxy::new(AsRef::<ot::Instance>::as_ref(self))?);
            }
        }
        Ok(())
    }

    pub fn is_advertising_proxy_enabled(&self) -> bool {
        self.srp_advertising_proxy.is_some()
    }

    pub fn set_advertising_proxy_enabled(&mut self, enabled: bool) -> Result {
        if self.is_advertising_proxy_enabled() != enabled {
            let _ = self.srp_advertising_proxy.take();
            if enabled {
                self.srp_advertising_proxy =
                    Some(AdvertisingProxy::new(AsRef::<ot::Instance>::as_ref(self))?);
            }
        }
        Ok(())
    }

    pub fn setup_border_agent_callback(&mut self)
    where
        OT: ot::InstanceInterface,
    {
        let (sender, receiver) = futures::channel::mpsc::unbounded();

        *self.border_agent.update_sender.lock() = Some(sender.clone());
        *self.border_agent.update_receiver.lock() = Some(receiver);

        self.ot_instance.border_agent_set_meshcop_service_changed_fn(Some(move || {
            trace!("Border agent MeshCoP service changed, triggering mDNS update");
            // NOTE: DRIVER STATE IS LOCKED WHEN THIS IS CALLED!
            // Send notification through channel (non-blocking, no reentrancy issues)
            let _ = sender.unbounded_send(());
        }));
    }

    pub fn setup_border_agent_ephemeral_key_callback(&mut self)
    where
        OT: ot::InstanceInterface,
    {
        let (sender, receiver) = futures::channel::mpsc::unbounded();

        *self.epskc.update_sender.lock() = Some(sender.clone());
        *self.epskc.update_receiver.lock() = Some(receiver);

        self.ot_instance.border_agent_set_ephemeral_key_callback(Some(move || {
            trace!("ePSKc mode state changed, triggering update");
            // NOTE: DRIVER STATE IS LOCKED WHEN THIS IS CALLED!
            // Send notification through channel (non-blocking, no reentrancy issues)
            let _ = sender.unbounded_send(());
        }));
    }
}

impl<OT: ot::BackboneRouter> DriverState<OT> {
    pub fn new(ot_instance: OT, net_if_id: NonZeroU64, backbone_if_id: Option<NonZeroU64>) -> Self {
        DriverState {
            ot_instance,
            connectivity_state: ConnectivityState::Inactive,
            srp_discovery_proxy: None,
            srp_advertising_proxy: None,
            ot_ctl: ot_ctl::OtCtl::new(),
            detailed_logging: detailed_logging::DetailedLogging::new(),
            nat64: nat64::Nat64::new(),
            dhcp_v6_pd: dhcpv6pd::DhcpV6Pd::default(),
            border_agent: border_agent::BorderAgent::new(),
            epskc: border_agent::Epskc::new(),
            multicast_routing_manager: backbone_if_id.map(move |backbone_if_id| {
                multicast_routing_manager::MulticastRoutingManager::new(net_if_id, backbone_if_id)
            }),
        }
    }
}

impl<OT> DriverState<OT>
where
    OT: Send + ot::InstanceInterface,
{
    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        self.connectivity_state.is_ready()
    }

    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.connectivity_state.is_active()
    }

    pub fn is_active_and_ready(&self) -> bool {
        self.connectivity_state.is_active_and_ready()
    }

    pub fn is_commissioning(&self) -> bool {
        self.connectivity_state.is_commissioning()
    }
}

impl<OT> DriverState<OT>
where
    OT: Send + ot::InstanceInterface,
{
    pub fn is_busy(&self) -> bool {
        self.ot_instance.is_energy_scan_in_progress()
            || self.ot_instance.is_active_scan_in_progress()
            || self.ot_instance.joiner_get_state() != ot::JoinerState::Idle
    }

    pub fn updated_connectivity_state(&self) -> ConnectivityState {
        let mut ret = self.connectivity_state;

        if self.ot_instance.is_commissioned() {
            ret = ret.provisioned();
        } else if self.ot_instance.joiner_get_state() == ot::JoinerState::Idle {
            ret = ret.unprovisioned();
        } else {
            ret = ret.commissioning().unwrap();
        }

        let role = self.get_current_role();

        ret = ret.role_updated(role);

        ret
    }

    pub fn get_current_role(&self) -> Role {
        match self.ot_instance.get_device_role() {
            ot::DeviceRole::Disabled => Role::Detached,
            ot::DeviceRole::Detached => Role::Detached,
            ot::DeviceRole::Child => Role::EndDevice,
            ot::DeviceRole::Router => Role::Router,
            ot::DeviceRole::Leader => Role::Leader,
        }
    }

    pub fn get_current_identity(&self) -> Identity {
        if !self.ot_instance.is_commissioned() {
            return Identity::default();
        }

        let mut operational_dataset = Default::default();
        match self.ot_instance.dataset_get_active(&mut operational_dataset) {
            Ok(()) => operational_dataset.into_ext(),
            Err(err) => {
                warn!("Commissioned, but unable to get active dataset: {:?}", err);
                Identity::default()
            }
        }
    }

    pub fn get_current_device_state(&self) -> DeviceState {
        DeviceState {
            connectivity_state: Some(self.updated_connectivity_state()),
            role: Some(self.get_current_role()),
            ..Default::default()
        }
    }

    pub fn is_initialized(&self) -> bool {
        // TODO: Evaluate, do we need this method?
        true
    }
}

impl<OT, NI, BI> OtDriver<OT, NI, BI>
where
    OT: Send + ot::InstanceInterface,
{
    /// Asynchronous task that waits for the given `DriverState`
    /// snapshot predicate closure to return true.
    ///
    /// If the predicate returns true, the task ends immediately.
    /// If the predicate returns false, the task will sleep until
    /// the next driver state change, upon which the predicate will
    /// be checked again.
    pub(crate) async fn wait_for_state<FN>(&self, predicate: FN)
    where
        FN: Fn(&DriverState<OT>) -> bool,
    {
        use std::ops::Deref;
        loop {
            {
                let driver_state = self.driver_state.lock();
                if predicate(driver_state.deref()) {
                    break;
                }
            }
            self.driver_state_change.wait().await;
        }
    }

    /// Called whenever the driver state has changed.
    pub(super) fn on_connectivity_state_change(
        &self,
        new_state: ConnectivityState,
        old_state: ConnectivityState,
    ) {
        info!("State Change: {:?} -> {:?}", old_state, new_state);

        self.driver_state_change.trigger();

        #[allow(clippy::match_single_binding)]
        match (old_state, new_state) {
            // TODO: Add state transition tasks here.

            // Unhandled state transition.
            (_, _) => {}
        }
    }

    pub fn update_connectivity_state(&self) {
        let mut driver_state = self.driver_state.lock();

        let new_connectivity_state = driver_state.updated_connectivity_state();

        if new_connectivity_state != driver_state.connectivity_state {
            let old_connectivity_state = driver_state.connectivity_state;
            driver_state.connectivity_state = new_connectivity_state;
            std::mem::drop(driver_state);
            self.driver_state_change.trigger();
            self.on_connectivity_state_change(new_connectivity_state, old_connectivity_state);
        }
    }

    pub(super) fn get_connectivity_state(&self) -> ConnectivityState {
        self.driver_state.lock().connectivity_state
    }
}
