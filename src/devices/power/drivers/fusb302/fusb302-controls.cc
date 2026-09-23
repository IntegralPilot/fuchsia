// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-controls.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <cstdint>
#include <utility>

#include "src/devices/power/drivers/fusb302/fusb302-sensors.h"
#include "src/devices/power/drivers/fusb302/registers.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"

namespace fusb302 {

Fusb302Controls::Fusb302Controls(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                                 Fusb302Sensors& sensors, inspect::Node root_node)
    : i2c_(i2c_channel),
      sensors_(sensors),
      root_node_(std::move(root_node)),
      wired_cc_pin_(&root_node_, "WiredCCPin", usb_pd::ConfigChannelPinSwitch::kNone),
      power_role_(&root_node_, "PowerRole", usb_pd::PowerRole::kSink),
      data_role_(&root_node_, "DataRole", usb_pd::DataRole::kUpstreamFacingPort),
      spec_revision_(&root_node_, "SpecRevision", usb_pd::SpecRevision::kRev2) {}

Fusb302Controls::~Fusb302Controls() = default;

zx::result<> Fusb302Controls::ResetIntoPowerRoleDiscovery() {
  fdf::trace("Configuring for hardware power role discovery");

  wired_cc_pin_.set(usb_pd::ConfigChannelPinSwitch::kNone);
  power_role_.set(usb_pd::PowerRole::kSink);
  data_role_.set(usb_pd::DataRole::kUpstreamFacingPort);
  spec_revision_.set(usb_pd::SpecRevision::kRev2);
  sensors_.SetConfiguration(power_role_.get(), wired_cc_pin_.get());

  zx::result<> result = PushStateToSwitchBlocks();
  if (result.is_error()) {
    return result;
  }
  result = PushStateToBmcPhyConfig();
  if (result.is_error()) {
    return result;
  }
  result = PushStateToPowerWells();
  if (result.is_error()) {
    return result;
  }
  result = PushStateToPdProtocolConfig();
  if (result.is_error()) {
    return result;
  }

  // Must be done after everything else is configured.
  return PushStateToPowerRoleDetectionControl();
}

zx::result<> Fusb302Controls::ConfigureAllRoles(usb_pd::ConfigChannelPinSwitch wired_cc_pin,
                                                usb_pd::PowerRole power_role,
                                                usb_pd::DataRole data_role,
                                                usb_pd::SpecRevision spec_revision) {
  fdf::debug("New Type C Port state: power role {}, Config Channel on {}",
             usb_pd::PowerRoleToString(power_role),
             usb_pd::ConfigChannelPinSwitchToString(wired_cc_pin));

  wired_cc_pin_.set(wired_cc_pin);
  power_role_.set(power_role);
  data_role_.set(data_role);
  spec_revision_.set(spec_revision);
  sensors_.SetConfiguration(power_role, wired_cc_pin);

  // Must be done first, so the hardware power role detection doesn't interfere
  // with our settings.
  zx::result<> result = PushStateToPowerRoleDetectionControl();
  if (result.is_error()) {
    return result.take_error();
  }

  // We don't use read-modify-write operations to act on Reset.
  zx_status_t status = ResetReg::Get().FromValue(0).set_pd_reset(true).WriteTo(i2c_);
  if (status != ZX_OK) {
    fdf::error("Failed to write Reset register: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  result = PushStateToSwitchBlocks();
  if (result.is_error()) {
    return result.take_error();
  }
  result = PushStateToBmcPhyConfig();
  if (result.is_error()) {
    return result.take_error();
  }
  result = PushStateToPowerWells();
  if (result.is_error()) {
    return result;
  }
  result = PushStateToPdProtocolConfig();
  if (result.is_error()) {
    return result;
  }

  return zx::ok();
}

zx::result<> Fusb302Controls::PushStateToPowerRoleDetectionControl() {
  return Control2Reg::ReadModifyWrite(i2c_, [&](Control2Reg& control2) {
    // TODO(costan): Detection mode should be configurable.
    const Fusb302RoleDetectionMode power_role_detection_mode =
        Fusb302RoleDetectionMode::kDualPowerRole;
    if (control2.mode() != power_role_detection_mode) {
      fdf::info("Changing automated power role detection mode from {} to {}",
                Fusb302RoleDetectionModeToString(control2.mode()),
                Fusb302RoleDetectionModeToString(power_role_detection_mode));
    }

    const bool automated_power_role_detection_enabled =
        (wired_cc_pin_.get() == usb_pd::ConfigChannelPinSwitch::kNone);
    if (control2.toggle() != automated_power_role_detection_enabled) {
      fdf::info("Changing automated power role detection from {} to {}",
                control2.toggle() ? "true" : "false",
                automated_power_role_detection_enabled ? "true" : "false");
    }

    control2.set_tog_save_pwr(Control2Reg::SleepDuration::k0ms)
        .set_tog_rd_only(true)
        .set_wake_en(false)
        .set_mode(power_role_detection_mode)
        .set_toggle(automated_power_role_detection_enabled);
  });
}

zx::result<> Fusb302Controls::PushStateToSwitchBlocks() {
  const usb_pd::ConfigChannelPinSwitch wired_pin_switch = wired_cc_pin_.get();

  // `kNone` means that we're configuring for power role detection, which mostly
  // means following Table 4 in the Rev 5 datasheet.
  //
  // Table 4 in the Rev 5 datasheet only demands that none of the CC pins is
  // connected to VCONN. We set SWITCHES0 to the reset value, which accomplishes
  // this requirement. The reset value has the additional benefit that it
  // advertises the Sink power role, so we'll remain powered throughout the
  // detection process.

  const usb_pd::ConfigChannelPinId wired_pin_id =
      (wired_pin_switch == usb_pd::ConfigChannelPinSwitch::kNone)
          ? usb_pd::ConfigChannelPinId::kCc1
          : ConfigChannelPinIdFromSwitch(wired_pin_switch);

  const usb_pd::ConfigChannelPinId other_pin_id = ConfigChannelPinIdFromInverse(wired_pin_id);

  ZX_DEBUG_ASSERT_MSG((wired_pin_switch != usb_pd::ConfigChannelPinSwitch::kNone) ||
                          power_role_.get() == usb_pd::PowerRole::kSink,
                      "The power role must be set to Sink during power role detection");

  const SwitchBlockConfig wired_pin_connection = (power_role_.get() == usb_pd::PowerRole::kSource)
                                                     ? SwitchBlockConfig::kPullUp
                                                     : SwitchBlockConfig::kPullDown;
  const SwitchBlockConfig other_pin_connection = (power_role_.get() == usb_pd::PowerRole::kSource)
                                                     ? SwitchBlockConfig::kConnectorVoltage
                                                     : SwitchBlockConfig::kPullDown;

  return Switches0Reg::ReadModifyWrite(i2c_, [&](Switches0Reg& switches0) {
    if (switches0.SwitchBlockConfigFor(wired_pin_id) != wired_pin_connection) {
      fdf::warn("Changing {} pin connection from {} of {}",
                ConfigChannelPinIdToString(wired_pin_id),
                SwitchBlockConfigToString(switches0.SwitchBlockConfigFor(wired_pin_id)),
                SwitchBlockConfigToString(wired_pin_connection));
    }

    if (switches0.SwitchBlockConfigFor(other_pin_id) != other_pin_connection) {
      fdf::warn("Changing {} pin connection from {} to {}",
                ConfigChannelPinIdToString(other_pin_id),
                SwitchBlockConfigToString(switches0.SwitchBlockConfigFor(other_pin_id)),
                SwitchBlockConfigToString(other_pin_connection));
    }

    if (switches0.MeasureBlockInput() != wired_pin_switch) {
      fdf::warn("Changing measure block input from {} to {}",
                ConfigChannelPinSwitchToString(switches0.MeasureBlockInput()),
                ConfigChannelPinSwitchToString(wired_pin_switch));
    }

    switches0.SetSwitchBlockConfig(wired_pin_id, wired_pin_connection)
        .SetSwitchBlockConfig(other_pin_id, other_pin_connection)
        .SetMeasureBlockInput(wired_pin_switch);
  });
}

zx::result<> Fusb302Controls::PushStateToBmcPhyConfig() {
  return Switches1Reg::ReadModifyWrite(i2c_, [&](Switches1Reg& switches1) {
    const usb_pd::ConfigChannelPinSwitch wired_pin_switch = wired_cc_pin_.get();
    if (switches1.BmcPhyConnection() != wired_pin_switch) {
      fdf::info("Changing BMC PHY CC pin connection from {} to {}",
                ConfigChannelPinSwitchToString(switches1.BmcPhyConnection()),
                ConfigChannelPinSwitchToString(wired_pin_switch));
    }

    const usb_pd::PowerRole power_role = power_role_.get();
    if (switches1.power_role() != power_role) {
      fdf::info("Changing GoodCRC header field 'Power Role' from {} to {}",
                usb_pd::PowerRoleToString(switches1.power_role()),
                usb_pd::PowerRoleToString(power_role));
    }

    const usb_pd::DataRole data_role = data_role_.get();
    if (switches1.data_role() != data_role) {
      fdf::info("Changing GoodCRC header field 'Data Role' from {} to {}",
                usb_pd::DataRoleToString(switches1.data_role()),
                usb_pd::DataRoleToString(data_role));
    }

    // Having the FUSB302 auto-reply with GoodCRC is necessary to meet the USB
    // PD timing requirements summarized below. The driver cannot send the
    // GoodCRC reply fast enough to avoid a Hard Reset, at least on Fuchsia,
    // during startup.
    //
    // USB PD spec Section 6.6.1 "CRCReceiveTime" gives us 195 us (tTransmit)
    // or 1 ms (tReceive) to start transmitting GoodCRC after receiving a
    // message, or we face a Soft Reset. Section 6.8.1 "Soft Reset and Protocol
    // Error" states that Soft Resets are upgraded to Hard Resets if they occur
    // during a voltage transition. Section 8.3.3.2.5 "PE_SRC_Transition_Supply
    // State" implies that the transition state includes the Accept and PS_RDY
    // messages.
    //
    // In our experiments, Type C ports and power supplies do follow this
    // interpretation of the spec. So, failing to acknowledge Accept or PS_RDY
    // with GoodCRC within 1ms leads to a Hard Reset.
    //
    // On the flip side, once the hardware auto-replies, Section 6.6.2 "Sender
    // Response Timer" gives us 15 ms (tSenderResponse) / 30 ms
    // (tReceiverResponse) to start transmitting replies to messages such as
    // Source_Capabilities.
    const bool generate_good_crc_replies =
        wired_pin_switch != usb_pd::ConfigChannelPinSwitch::kNone;
    if (switches1.auto_crc() != generate_good_crc_replies) {
      fdf::info("Changing 'Generate GoodCRC replies' from {} to {}",
                switches1.auto_crc() ? "true" : "false",
                generate_good_crc_replies ? "true" : "false");
    }

    switches1.set_power_role(power_role)
        .set_spec_rev(spec_revision_.get())
        .set_data_role(data_role)
        .set_auto_crc(generate_good_crc_replies)
        .SetBmcPhyConnection(wired_pin_switch);
  });
}

zx::result<> Fusb302Controls::PushStateToPowerWells() {
  // Table 4 in the Rev 5 datasheet states that power wells 1-3 are needed for
  // automated power role detection. We need power well 4 enabled to reply to PD
  // messages once we have a Config Channel connection.
  //
  // TOOD(costan): We can receive without well 4. Can we modulate it on TX?
  const bool oscillator_powered_on = true;

  return PowerReg::ReadModifyWrite(i2c_, [&](PowerReg& power) {
    power.set_pwr3(oscillator_powered_on).set_pwr2(true).set_pwr1(true).set_pwr0(true);
  });
}

zx::result<> Fusb302Controls::PushStateToPdProtocolConfig() {
  // We support two configurations: power role discovery, and PD communications
  // from a known Type C port configuration. Our power role discovery
  // configuration follows the guidelines in Table 4 of the Rev 5 datasheet, but
  // we also set all other documented configuration fields.
  const bool power_role_discovery = wired_cc_pin_.get() == usb_pd::ConfigChannelPinSwitch::kNone;
  const bool bmc_phy_enabled = !power_role_discovery;

  // When we support host mode, the pull-up in the host role (last value) should
  // become configurable,
  const Control0Reg::PullUpCurrent pull_up_current =
      power_role_discovery ? Control0Reg::PullUpCurrent::kUsbStandard_80uA
                           : (power_role_.get() == usb_pd::PowerRole::kSink
                                  ? Control0Reg::PullUpCurrent::kNone
                                  : Control0Reg::PullUpCurrent::kUsbStandard_80uA);

  // Table 4 only demands that the pull-up current source is set to Standard USB
  // power, and that interrupts are enabled. We use the reset configuration in
  // power role discovery, and we kick off a Rx (receiver) FIFO flush if we're
  // preparing for PD communication.
  zx::result<> result = Control0Reg::ReadModifyWrite(i2c_, [&](Control0Reg& control0) {
    control0.set_tx_flush(bmc_phy_enabled)
        .set_int_mask(false)
        .set_host_cur(pull_up_current)
        .set_auto_pre(false)
        .set_tx_start(false);
  });
  if (result.is_error()) {
    return result;
  }

  // This register is not mentioned in Table 4. We use the reset configuration
  // in power role discovery, and we kick off a Rx (receiver) FIFO flush if
  // we're preparing for PD communication.
  result = Control1Reg::ReadModifyWrite(i2c_, [&](Control1Reg& control1) {
    control1.set_ensop2db(false)
        .set_ensop1db(false)
        .set_bist_mode2(false)
        .set_rx_flush(bmc_phy_enabled)
        .set_ensop2(false)
        .set_ensop1(false);
  });
  if (result.is_error()) {
    return result;
  }

  // This register is not mentioned in Table 4. We use the reset configuration,
  // but enable re-transmission on GoodCRC timeout.
  result = Control3Reg::ReadModifyWrite(i2c_, [&](Control3Reg& control3) {
    const bool retry_sending_on_good_crc_timeout = true;

    if (control3.auto_retry() != retry_sending_on_good_crc_timeout) {
      fdf::info("Changing 'Retry sending on GoodCRC timeout' from {} to {}",
                control3.auto_retry() ? "true" : "false",
                retry_sending_on_good_crc_timeout ? "true" : "false");
    }

    control3.set_send_hard_reset(false)
        .set_bist_tmode(false)
        .set_auto_hardreset(false)
        .set_auto_softreset(false)
        .set_n_retries(3)
        .set_auto_retry(true);
  });
  if (result.is_error()) {
    return result;
  }

  // This register is not mentioned in Table 4. We set it here because it
  // configures power role discovery. In particular, we don't want the discovery
  // process to conclude we're attached to an Audio Accessory.
  result = Control4Reg::ReadModifyWrite(
      i2c_, [&](Control4Reg& control4) { control4.set_tog_exit_aud(false); });
  if (result.is_error()) {
    return result;
  }

  // Table 4 only demands that the measure block is set up to measure CC pins,
  // as opposed to VBUS. We reduce the state space by setting the reference
  // voltage to 2.05 V, which is recommended by Table 5 for distinguishing
  // between Source terminations.
  result = MeasureReg::ReadModifyWrite(i2c_, [&](MeasureReg& measure) {
    measure.set_meas_vbus(false).SetComparatorVoltageMv(2'058);
  });
  if (result.is_error()) {
    return result;
  }

  if (bmc_phy_enabled) {
    result = WaitForTransmitQueueFlush();
    if (result.is_error()) {
      return result;
    }
    result = WaitForReceiveQueueFlush();
    if (result.is_error()) {
      return result;
    }
  }

  return zx::ok();
}

zx::result<> Fusb302Controls::WaitForTransmitQueueFlush() {
  for (int retry_count = 0; retry_count < 10; ++retry_count) {
    if (!Control0Reg::ReadFrom(i2c_).tx_flush()) {
      return zx::ok();
    }
    zx::nanosleep(zx::deadline_after(zx::usec(10)));
  }

  fdf::error("Timed out waiting for the transmit queue to flush");
  return zx::error(ZX_ERR_TIMED_OUT);
}

zx::result<> Fusb302Controls::WaitForReceiveQueueFlush() {
  for (int retry_count = 0; retry_count < 10; ++retry_count) {
    if (!Control1Reg::ReadFrom(i2c_).rx_flush()) {
      return zx::ok();
    }
    zx::nanosleep(zx::deadline_after(zx::usec(10)));
  }

  fdf::error("Timed out waiting for the receive queue to flush");
  return zx::error(ZX_ERR_TIMED_OUT);
}

}  // namespace fusb302
