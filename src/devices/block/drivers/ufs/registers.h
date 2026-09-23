// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_REGISTERS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_REGISTERS_H_

#include <algorithm>

#include <hwreg/bitfields.h>

#include "src/devices/block/drivers/ufs/uic/uic_commands.h"

namespace ufs {

// UFSHCI Specification Version 3.0, section 5.1 "Register Map".
enum RegisterMap {
  // Host Capabilities
  kCAP = 0x00,
  kVER = 0x08,
  kHCDDID = 0x10,
  kHCPMID = 0x14,
  kAHIT = 0x18,
  // Operation and Runtime
  kIS = 0x20,
  kIE = 0x24,
  kHCS = 0x30,
  kHCE = 0x34,
  kUECPA = 0x38,
  kUECDL = 0x3C,
  kUECN = 0x40,
  kUECT = 0x44,
  kUECDME = 0x48,
  kUTRIACR = 0x4C,
  // UTP Transfer Request
  kUTRLBA = 0x50,
  kUTRLBAU = 0x54,
  kUTRLDBR = 0x58,
  kUTRLCLR = 0x5c,
  kUTRLRSR = 0x60,
  kUTRLCNR = 0x64,
  // UTP Task Management
  kUTMRLBA = 0x70,
  kUTMRLBAU = 0x74,
  kUTMRLDBR = 0x78,
  kUTMRLCLR = 0x7c,
  kUTMRLRSR = 0x80,
  // UIC Command
  kUICCMD = 0x90,
  kUICCMDARG1 = 0x94,
  kUICCMDARG2 = 0x98,
  kUICCMDARG3 = 0x9c,
  // Crypto
  kCCAP = 0x100,
  kRegisterSize = 0x104,
};

// UFSHCI Specification Version 3.0, section 5.2.1
// "Offset 00h: CAP – Controller Capabilities".
class CapabilityReg : public hwreg::RegisterBase<CapabilityReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(28, crypto_support);
  DEF_BIT(26, uic_dme_test_mode_command_supported);
  DEF_BIT(25, out_of_order_data_delivery_supported);
  DEF_BIT(24, _64_bit_addressing_supported);
  DEF_BIT(23, auto_hibernation_support);
  DEF_FIELD(18, 16, number_of_utp_task_management_request_slots);  // 0's based value
  DEF_FIELD(15, 8, number_of_outstanding_rtt_requests_supported);
  DEF_FIELD(4, 0, number_of_utp_transfer_request_slots);  // 0's based value

  static auto Get() { return hwreg::RegisterAddr<CapabilityReg>(RegisterMap::kCAP); }
};

// UFSHCI Specification Version 3.0, section 5.2.2
// "Offset 08h: VER – UFS Version".
class VersionReg : public hwreg::RegisterBase<VersionReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(15, 8, major_version_number);
  DEF_FIELD(7, 4, minor_version_number);
  DEF_FIELD(3, 0, version_suffix);

  static auto Get() { return hwreg::RegisterAddr<VersionReg>(RegisterMap::kVER); }
};

// UFSHCI Specification Version 3.0, section 5.2.5
// "Offset 18h: AHIT – Auto-Hibernate Idle Timer".
class AutoHibernateIdleTimerReg
    : public hwreg::RegisterBase<AutoHibernateIdleTimerReg, uint32_t, hwreg::EnablePrinter> {
 public:
  enum class Scale : uint32_t {
    k1us = 0,
    k10us = 1,
    k100us = 2,
    k1ms = 3,
    k10ms = 4,
    k100ms = 5,
  };

  DEF_FIELD(9, 0, timer_value);
  DEF_ENUM_FIELD(Scale, 12, 10, timer_scale);

  static constexpr uint32_t kMaxTimerValue = 1023;

  static auto Get() { return hwreg::RegisterAddr<AutoHibernateIdleTimerReg>(RegisterMap::kAHIT); }

  static AutoHibernateIdleTimerReg FromTimeoutUs(uint32_t timeout_us) {
    uint32_t scale = static_cast<uint32_t>(Scale::k1us);
    uint32_t unit_us = 1;
    while (scale < static_cast<uint32_t>(Scale::k100ms) && timeout_us > kMaxTimerValue * unit_us) {
      unit_us *= 10;
      scale++;
    }
    uint32_t timer_val = std::min(timeout_us / unit_us, kMaxTimerValue);
    return AutoHibernateIdleTimerReg::Get()
        .FromValue(0)
        .set_timer_scale(static_cast<Scale>(scale))
        .set_timer_value(timer_val);
  }
};

// UFSHCI Specification Version 3.0, section 5.3.1
// "Offset 20h: IS – Interrupt Status".
class InterruptStatusReg
    : public hwreg::RegisterBase<InterruptStatusReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(18, crypto_engine_fatal_error_status);
  DEF_BIT(17, system_bus_fatal_error_status);
  DEF_BIT(16, host_controller_fatal_error_status);
  DEF_BIT(12, utp_error_status);
  DEF_BIT(11, device_fatal_error_status);
  DEF_BIT(10, uic_command_completion_status);
  DEF_BIT(9, utp_task_management_request_completion_status);
  DEF_BIT(8, uic_link_startup_status);
  DEF_BIT(7, uic_link_lost_status);
  DEF_BIT(6, uic_hibernate_enter_status);
  DEF_BIT(5, uic_hibernate_exit_status);
  DEF_BIT(4, uic_power_mode_status);
  DEF_BIT(3, uic_test_mode_status);
  DEF_BIT(2, uic_error);
  DEF_BIT(1, uic_dme_endpointreset_indication);
  DEF_BIT(0, utp_transfer_request_completion_status);

  static auto Get() { return hwreg::RegisterAddr<InterruptStatusReg>(RegisterMap::kIS); }
};

// UFSHCI Specification Version 3.0, section 5.3.2
// "Offset 24h: IE – Interrupt Enable".
class InterruptEnableReg
    : public hwreg::RegisterBase<InterruptEnableReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(18, crypto_engine_fatal_error_enable);
  DEF_BIT(17, system_bus_fatal_error_enable);
  DEF_BIT(16, host_controller_fatal_error_enable);
  DEF_BIT(12, utp_error_enable);
  DEF_BIT(11, device_fatal_error_enable);
  DEF_BIT(10, uic_command_completion_enable);
  DEF_BIT(9, utp_task_management_request_completion_enable);
  DEF_BIT(8, uic_link_startup_status_enable);
  DEF_BIT(7, uic_link_lost_status_enable);
  DEF_BIT(6, uic_hibernate_enter_status_enable);
  DEF_BIT(5, uic_hibernate_exit_status_enable);
  DEF_BIT(4, uic_power_mode_status_enable);
  DEF_BIT(3, uic_test_mode_status_enable);
  DEF_BIT(2, uic_error_enable);
  DEF_BIT(1, uic_dme_endpointreset);
  DEF_BIT(0, utp_transfer_request_completion_enable);

  static auto Get() { return hwreg::RegisterAddr<InterruptEnableReg>(RegisterMap::kIE); }
};

// UFSHCI Specification Version 3.0, section 5.3.3
// "Offset 30h: HCS – Host Controller Status".
class HostControllerStatusReg
    : public hwreg::RegisterBase<HostControllerStatusReg, uint32_t, hwreg::EnablePrinter> {
 public:
  enum ErrorCode {
    kRejectUpiuHasInvalidTaskTagOrLun = 0,
    kInvalidUpiuType,
    kTrUpiuHasInvalidTaskTagOrLun,
    kTmrUpiuHasInvalidTaskTagOrLun,
  };
  enum PowerModeStatus {
    kPowerOk = 0,
    kPowerLocal,
    kPowerRemote,
    kPowerBusy,
    kPowerErrorCap,
    kPowerFatalError,
  };
  DEF_FIELD(31, 24, target_lun_of_utp_error);
  DEF_FIELD(23, 16, task_tag_of_utp_error);
  DEF_ENUM_FIELD(ErrorCode, 15, 12, utp_error_code);
  DEF_ENUM_FIELD(PowerModeStatus, 10, 8, uic_power_mode_change_request_status);
  DEF_BIT(3, uic_command_ready);
  DEF_BIT(2, utp_task_management_request_list_ready);
  DEF_BIT(1, utp_transfer_request_list_ready);
  DEF_BIT(0, device_present);

  static auto Get() { return hwreg::RegisterAddr<HostControllerStatusReg>(RegisterMap::kHCS); }
};

// UFSHCI Specification Version 3.0, section 5.3.4
// "Offset 34h: HCE – Host Controller Enable".
class HostControllerEnableReg
    : public hwreg::RegisterBase<HostControllerEnableReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(1, crypto_general_enable);
  DEF_BIT(0, host_controller_enable);

  static auto Get() { return hwreg::RegisterAddr<HostControllerEnableReg>(RegisterMap::kHCE); }
};

// UFSHCI Specification Version 3.0, section 5.3.5
// "Offset 38h: UECPA – Host UIC Error Code PHY Adapter Layer".
class HostUicErrorCodePhyAdapterLayerReg
    : public hwreg::RegisterBase<HostUicErrorCodePhyAdapterLayerReg, uint32_t,
                                 hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, uic_phy_adapter_layer_error);
  DEF_FIELD(4, 0, uic_phy_adapter_layer_error_code);

  static auto Get() {
    return hwreg::RegisterAddr<HostUicErrorCodePhyAdapterLayerReg>(RegisterMap::kUECPA);
  }
};

// UFSHCI Specification Version 3.0, section 5.3.6
// "Offset 3Ch: UECDL – Host UIC Error Code Data Link Layer".
class HostUicErrorCodeDataLinkLayerReg
    : public hwreg::RegisterBase<HostUicErrorCodeDataLinkLayerReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, uic_data_link_layer_error);
  DEF_FIELD(15, 0, uic_data_link_layer_error_code);

  static auto Get() {
    return hwreg::RegisterAddr<HostUicErrorCodeDataLinkLayerReg>(RegisterMap::kUECDL);
  }
};

// UFSHCI Specification Version 3.0, section 5.3.7
// "Offset 40h: UECN – Host UIC Error Code Network Layer".
class HostUicErrorCodeNetworkLayerReg
    : public hwreg::RegisterBase<HostUicErrorCodeNetworkLayerReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, uic_network_layer_error);
  DEF_FIELD(2, 0, uic_network_layer_error_code);

  static auto Get() {
    return hwreg::RegisterAddr<HostUicErrorCodeNetworkLayerReg>(RegisterMap::kUECN);
  }
};

// UFSHCI Specification Version 3.0, section 5.3.8
// "Offset 44h: UECT – Host UIC Error Code Transport Layer".
class HostUicErrorCodeTransportLayerReg
    : public hwreg::RegisterBase<HostUicErrorCodeTransportLayerReg, uint32_t,
                                 hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, uic_transport_layer_error);
  DEF_FIELD(6, 0, uic_transport_layer_error_code);

  static auto Get() {
    return hwreg::RegisterAddr<HostUicErrorCodeTransportLayerReg>(RegisterMap::kUECT);
  }
};

// UFSHCI Specification Version 3.0, section 5.3.9
// "Offset 48h: UECDME – Host UIC Error Code".
class HostUicErrorCodeReg
    : public hwreg::RegisterBase<HostUicErrorCodeReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(31, uic_dme_error);
  DEF_FIELD(3, 0, uic_dme_error_code);

  static auto Get() { return hwreg::RegisterAddr<HostUicErrorCodeReg>(RegisterMap::kUECDME); }
};

// UFSHCI Specification Version 3.0, section 5.4.1
// "Offset 50h: UTRLBA – UTP Transfer Request List Base Address".
class UtrListBaseAddressReg
    : public hwreg::RegisterBase<UtrListBaseAddressReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, address);

  static auto Get() { return hwreg::RegisterAddr<UtrListBaseAddressReg>(RegisterMap::kUTRLBA); }
};

// UFSHCI Specification Version 3.0, section 5.4.2
// "Offset 54h: UTRLBAU – UTP Transfer Request List Base Address Upper 32-bits".
class UtrListBaseAddressUpperReg
    : public hwreg::RegisterBase<UtrListBaseAddressUpperReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, address_upper);

  static auto Get() {
    return hwreg::RegisterAddr<UtrListBaseAddressUpperReg>(RegisterMap::kUTRLBAU);
  }
};

// UFSHCI Specification Version 3.0, section 5.4.3
// "Offset 58h: UTRLDBR – UTP Transfer Request List Door Bell Register".
class UtrListDoorBellReg
    : public hwreg::RegisterBase<UtrListDoorBellReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, door_bell);

  static auto Get() { return hwreg::RegisterAddr<UtrListDoorBellReg>(RegisterMap::kUTRLDBR); }
};

// UFSHCI Specification Version 3.0, section 5.4.4
// "Offset 5Ch: UTRLCLR – UTP Transfer Request List Clear Register".
class UtrListClearReg
    : public hwreg::RegisterBase<UtrListClearReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, clear);

  static auto Get() { return hwreg::RegisterAddr<UtrListClearReg>(RegisterMap::kUTRLCLR); }
};

// UFSHCI Specification Version 3.0, section 5.4.5
// "Offset 60h: UTRLRSR – UTP Transfer Request List Run Stop Register".
class UtrListRunStopReg
    : public hwreg::RegisterBase<UtrListRunStopReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(0, value);

  static auto Get() { return hwreg::RegisterAddr<UtrListRunStopReg>(RegisterMap::kUTRLRSR); }
};

// UFSHCI Specification Version 3.0, section 5.4.6
// "Offset 64h: UTRLCNR – UTP Transfer Request List Completion Notification Register".
class UtrListCompletionNotificationReg
    : public hwreg::RegisterBase<UtrListCompletionNotificationReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, notification);

  static auto Get() {
    return hwreg::RegisterAddr<UtrListCompletionNotificationReg>(RegisterMap::kUTRLCNR);
  }
};

// UFSHCI Specification Version 3.0, section 5.5.1
// "Offset 70h: UTMRLBA – UTP Task Management Request List Base Address".
class UtmrListBaseAddressReg
    : public hwreg::RegisterBase<UtmrListBaseAddressReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, address);

  static auto Get() { return hwreg::RegisterAddr<UtmrListBaseAddressReg>(RegisterMap::kUTMRLBA); }
};

// UFSHCI Specification Version 3.0, section 5.5.2
// "Offset 74h: UTMRLBAU – UTP Task Management Request List Base Address Upper 32-bits".
class UtmrListBaseAddressUpperReg
    : public hwreg::RegisterBase<UtmrListBaseAddressUpperReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, address_upper);

  static auto Get() {
    return hwreg::RegisterAddr<UtmrListBaseAddressUpperReg>(RegisterMap::kUTMRLBAU);
  }
};

// UFSHCI Specification Version 3.0, section 5.5.3
// "Offset 78h: UTMRLDBR – UTP Task Management Request List Door Bell Register".
class UtmrListDoorBellReg
    : public hwreg::RegisterBase<UtmrListDoorBellReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(7, 0, door_bell);

  static auto Get() { return hwreg::RegisterAddr<UtmrListDoorBellReg>(RegisterMap::kUTMRLDBR); }
};

// UFSHCI Specification Version 3.0, section 5.5.5
// "Offset 80h: UTMRLRSR – UTP Task Management Request List Run Stop Register".
class UtmrListRunStopReg
    : public hwreg::RegisterBase<UtmrListRunStopReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_BIT(0, value);

  static auto Get() { return hwreg::RegisterAddr<UtmrListRunStopReg>(RegisterMap::kUTMRLRSR); }
};

// UFSHCI Specification Version 3.0, section 5.6.1
// "Offset 90h: UICCMD – UIC Command".
class UicCommandReg : public hwreg::RegisterBase<UicCommandReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_ENUM_FIELD(UicCommandOpcode, 7, 0, command_opcode);

  static auto Get() { return hwreg::RegisterAddr<UicCommandReg>(RegisterMap::kUICCMD); }
};

// UFSHCI Specification Version 3.0, section 5.6.2
// "Offset 94h: UICCMDARG1 – UIC Command Argument 1".
class UicCommandArgument1Reg
    : public hwreg::RegisterBase<UicCommandArgument1Reg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 16, mib_attribute);
  DEF_FIELD(15, 0, gen_selector_index);

  static auto Get() {
    return hwreg::RegisterAddr<UicCommandArgument1Reg>(RegisterMap::kUICCMDARG1);
  }
};

// UFSHCI Specification Version 3.0, section 5.6.3
// "Offset 98h: UICCMDARG2 – UIC Command Argument 2".
class UicCommandArgument2Reg
    : public hwreg::RegisterBase<UicCommandArgument2Reg, uint32_t, hwreg::EnablePrinter> {
 public:
  enum GenericErrorCode {
    kSuccess = 0,
    kFailure = 1,
  };

  DEF_ENUM_FIELD(AttrSetType, 23, 16, attr_set_type);
  // Bits 7 to 1 are reserved. |result_code| is also used as a ConfigResultCode, but currently has
  // no use, so we only use it as a GenericErrorCode.
  DEF_ENUM_FIELD(GenericErrorCode, 7, 0, result_code);

  static auto Get() {
    return hwreg::RegisterAddr<UicCommandArgument2Reg>(RegisterMap::kUICCMDARG2);
  }
};

// UFSHCI Specification Version 3.0, section 5.6.4
// "Offset 9Ch: UICCMDARG3 – UIC Command Argument 3".
class UicCommandArgument3Reg
    : public hwreg::RegisterBase<UicCommandArgument3Reg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_FIELD(31, 0, value);

  static auto Get() {
    return hwreg::RegisterAddr<UicCommandArgument3Reg>(RegisterMap::kUICCMDARG3);
  }
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_REGISTERS_H_
