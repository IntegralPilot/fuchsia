// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/abr/abr.h>
#include <lib/zircon_boot/zircon_boot.h>
#include <zircon_boot_ops.h>

#include <cstdint>
#include <optional>

#include <gbl/gbl_efi_protocols.h>
#include <gbl/uefi/gbl_protocol_utils.h>
#include <gbl/uefi/protocols/gbl_efi_boot_control_protocol.h>
#include <phys/efi/main.h>

#include "efi/types.h"

#define GBL_EFI_BOOT_CONTROL_PROTOCOL_GUID \
  {0xd382db1b, 0x9ac2, 0x11f0, {0x84, 0xc7, 0x04, 0x7b, 0xcb, 0xa9, 0x60, 0x19}}

// Set within `LaunchGbl()`;
bool g_should_stop_in_fastboot = false;

namespace {

GblEfiUnbootableReason abr_to_gbl_unbootable_reason(AbrUnbootableReason reason) {
  switch (reason) {
    case kAbrUnbootableReasonNoMoreTries:
      return GblEfiUnbootableReason::GBL_EFI_UNBOOTABLE_REASON_NO_MORE_TRIES;
    case kAbrUnbootableReasonOsRequested:
      return GblEfiUnbootableReason::GBL_EFI_UNBOOTABLE_REASON_USER_REQUESTED;
    case kAbrUnbootableReasonVerificationFailure:
      return GblEfiUnbootableReason::GBL_EFI_UNBOOTABLE_REASON_VERIFICATION_FAILURE;
    case kAbrUnbootableReasonNone:
    default:
      return GblEfiUnbootableReason::GBL_EFI_UNBOOTABLE_REASON_UNKNOWN_REASON;
  }
}

std::optional<AbrSlotIndex> int_to_abr_index(uint8_t index) {
  switch (index) {
    case 0:
      return kAbrSlotIndexA;
    case 1:
      return kAbrSlotIndexB;
    default:
      return std::nullopt;
  };
}

std::optional<uint8_t> abr_index_to_int(AbrSlotIndex abr_index) {
  switch (abr_index) {
    case kAbrSlotIndexA:
      return 0;
    case kAbrSlotIndexB:
      return 1;
    default:
      return std::nullopt;
  };
}

// Slot metadata query methods
EfiStatus GetSlotCount(struct GblEfiBootControlProtocol* self,
                       /* out */ uint8_t* slot_count) {
  if (!slot_count) {
    return EFI_INVALID_PARAMETER;
  }

  *slot_count = 2;
  return EFI_SUCCESS;
}

// Boot control methods
EfiStatus GetOneShotBootMode(struct GblEfiBootControlProtocol* self, GblEfiOneShotBootMode* mode) {
  if (!mode) {
    return EFI_INVALID_PARAMETER;
  }
  *mode = g_should_stop_in_fastboot ? GblEfiOneShotBootMode::GBL_EFI_ONE_SHOT_BOOT_MODE_BOOTLOADER
                                    : GblEfiOneShotBootMode::GBL_EFI_ONE_SHOT_BOOT_MODE_NONE;
  return EFI_SUCCESS;
}

EfiStatus GetSlotInfo(struct GblEfiBootControlProtocol* self, uint8_t index, GblEfiSlotInfo* info) {
  if (!info) {
    return EFI_INVALID_PARAMETER;
  }

  ZirconBootOps zb_ops = gigaboot::GetZirconBootOps();
  AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
  const auto abr_index = int_to_abr_index(index);
  if (!abr_index.has_value()) {
    return EFI_INVALID_PARAMETER;
  }

  AbrSlotInfo abr_info;
  AbrResult res = AbrGetSlotInfo(&abr_ops, abr_index.value(), &abr_info);
  if (res != kAbrResultOk) {
    return EFI_DEVICE_ERROR;
  }

  *info = {
      .suffix = static_cast<uint32_t>(AbrGetSlotSuffix(abr_index.value())[1]),
      .unbootable_reason = abr_to_gbl_unbootable_reason(
          static_cast<AbrUnbootableReason>(abr_info.unbootable_reason)),
      // AbrSlotInfo doesn't track priority directly,
      // so hack up a reasonable, normalized priority value.
      .priority = static_cast<uint8_t>(abr_info.is_active ? 7 : 6),
      .remaining_tries = abr_info.num_tries_remaining,
      .successful = static_cast<uint8_t>(abr_info.is_marked_successful ? 1 : 0),
  };

  return EFI_SUCCESS;
}

EfiStatus GetCurrentSlot(struct GblEfiBootControlProtocol* self, GblEfiSlotInfo* info) {
  if (!info) {
    return EFI_INVALID_PARAMETER;
  }

  ZirconBootOps zb_ops = gigaboot::GetZirconBootOps();
  AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
  AbrSlotIndex abr_index = AbrGetBootSlot(&abr_ops, false, nullptr);
  const auto idx = abr_index_to_int(abr_index);
  if (!idx.has_value()) {
    return EFI_INVALID_PARAMETER;
  }

  return GetSlotInfo(self, idx.value(), info);
}

EfiStatus SetActiveSlot(struct GblEfiBootControlProtocol* self, uint8_t index) {
  const auto abr_index = int_to_abr_index(index);
  if (!abr_index.has_value()) {
    return EFI_INVALID_PARAMETER;
  }

  ZirconBootOps zb_ops = gigaboot::GetZirconBootOps();
  AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
  AbrResult res = AbrMarkSlotActive(&abr_ops, abr_index.value());
  return (res == kAbrResultOk) ? EFI_SUCCESS : EFI_DEVICE_ERROR;
}

GblEfiBootControlProtocol protocol = {
    .revision = GBL_EFI_BOOT_CONTROL_PROTOCOL_REVISION,
    .get_slot_count = GetSlotCount,
    .get_slot_info = GetSlotInfo,
    .get_current_slot = GetCurrentSlot,
    .set_active_slot = SetActiveSlot,
    .get_one_shot_boot_mode = GetOneShotBootMode,
    .handle_loaded_os = nullptr,
};

efi_guid guid = GBL_EFI_BOOT_CONTROL_PROTOCOL_GUID;

}  // namespace

namespace gigaboot {
efi_status InstallGblEfiBootControlProtocol() {
  efi_handle out_handle = nullptr;
  return gEfiSystemTable->BootServices->InstallMultipleProtocolInterfaces(&out_handle, &guid,
                                                                          &protocol, NULL);
}
}  // namespace gigaboot
