// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;

use futures::prelude::*;

use log::{debug, error, info, trace, warn};
use lowpan_driver_common::spinel::Subnet;

impl<OT, NI, BI> OtDriver<OT, NI, BI>
where
    OT: Send + ot::InstanceInterface,
    NI: NetworkInterface,
    BI: BackboneInterface,
{
    pub fn on_ot_ip_receive(
        &self,
        msg: OtMessageBox<'_>,
        frame_type: fidl_fuchsia_hardware_network::FrameType,
    ) {
        // NOTE: DRIVER STATE IS ALREADY LOCKED WHEN THIS IS CALLED!
        //       Calling `lock()` on the driver state will deadlock!

        if !msg.is_link_security_enabled() {
            // TODO: Check firewall.
            return;
        }

        // Unfortunately we must render the packet out before we can pass it along.
        let packet = msg.to_vec();

        if let Err(err) =
            self.net_if.inbound_packet_to_stack(&packet, frame_type).now_or_never().transpose()
        {
            error!("Unable to send packet to netstack: {:?}", err);
        }
    }

    pub(crate) async fn on_ot_state_change(
        &self,
        flags: ot::ChangedFlags,
    ) -> Result<(), anyhow::Error> {
        debug!("OpenThread State Change: {:?}", flags);
        self.update_connectivity_state();

        // TODO(rquattle): Consider make this a little more selective, this async-condition
        //                 is a bit of a big hammer.
        if flags.intersects(
            ot::ChangedFlags::THREAD_NETWORK_NAME
                | ot::ChangedFlags::THREAD_CHANNEL
                | ot::ChangedFlags::THREAD_PANID
                | ot::ChangedFlags::THREAD_EXT_PANID
                | ot::ChangedFlags::THREAD_ROLE
                | ot::ChangedFlags::JOINER_STATE
                | ot::ChangedFlags::ACTIVE_DATASET,
        ) {
            self.driver_state_change.trigger();
        }

        if flags.intersects(
            ot::ChangedFlags::THREAD_ROLE
                | ot::ChangedFlags::THREAD_EXT_PANID
                | ot::ChangedFlags::THREAD_NETWORK_NAME
                | ot::ChangedFlags::ACTIVE_DATASET
                | ot::ChangedFlags::THREAD_PARTITION_ID
                | ot::ChangedFlags::THREAD_BACKBONE_ROUTER_STATE,
        ) {
            self.driver_state.lock().border_agent.trigger_service_update();
        }

        if flags.intersects(ot::ChangedFlags::NAT64_TRANSLATOR_STATE) {
            self.on_nat64_translator_state_changed();
        }

        {
            let driver_state = self.driver_state.lock();
            driver_state.ot_instance.on_radio_handle_state_change(flags);
        }

        Ok(())
    }

    pub(crate) fn on_ot_ip6_address_info(&self, info: ot::Ip6AddressInfo<'_>, is_added: bool) {
        // NOTE: DRIVER STATE IS LOCKED WHEN THIS IS CALLED!
        //       Calling `lock()` on the driver state will deadlock!

        trace!("on_ot_ip6_address_info: is_added:{} {:?}", is_added, info);

        let subnet = Subnet { addr: *info.addr(), prefix_len: info.prefix_len() };
        if info.is_multicast() {
            if is_added {
                debug!("OpenThread JOINED multicast group: {:?}", info);
                if let Err(err) = self.net_if.join_mcast_group(info.addr()).ignore_already_exists()
                {
                    warn!("Unable to join multicast group `{:?}`: {:?}", subnet, err);
                }
            } else {
                debug!("OpenThread LEFT multicast group: {:?}", info);
                if let Err(err) = self.net_if.leave_mcast_group(info.addr()).ignore_not_found() {
                    warn!("Unable to leave multicast group `{:?}`: {:?}", subnet, err);
                }
            }
        } else if is_added {
            debug!("OpenThread ADDED address: {:?}", info);

            // TODO(b/235498515): If it looks like an RLOC, don't add it for the time being.
            if subnet.addr.segments()[4..7] == [0x0, 0xff, 0xfe00] {
                info!(
                    "HACK(b/235498515): Refusing to add {:?} because it looks like an RLOC",
                    subnet.addr
                );
            } else if let Err(err) =
                self.net_if.add_address_from_spinel_subnet(&subnet).ignore_already_exists()
            {
                warn!("Unable to add address `{:?}` to interface: {:?}", subnet, err);
            }
        } else {
            debug!("OpenThread REMOVED address: {:?}", info);
            if subnet.addr.segments()[4..7] == [0x0, 0xff, 0xfe00] {
                info!(
                    "HACK(b/235498515): Refusing to remove {:?} because it looks like an RLOC",
                    subnet.addr
                );
            } else if let Err(err) =
                self.net_if.remove_address_from_spinel_subnet(&subnet).ignore_not_found()
            {
                warn!("Unable to remove address `{:?}` from interface: {:?}", subnet, err);
            }
        }
    }
}
