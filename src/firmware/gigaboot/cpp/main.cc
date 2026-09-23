// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "phys/efi/main.h"

#include <lib/abr/abr.h>
#include <stdio.h>

#include <optional>
#include <string_view>

#include <phys/stdio.h>

#include "backends.h"
#include "fastboot_tcp.h"
#include "gbl_loader.h"
#include "gigaboot/src/netifc.h"
#include "input.h"
#include "lib/zircon_boot/zircon_boot.h"
#include "utils.h"
#include "xefi.h"
#include "zircon_boot_ops.h"

#if defined(__x86_64__)
#include <cpuid.h>
#endif

// This is necessary because the Fastboot class inherits from FastbootBase,
// which is an abstract class with pure virtual functions.
// The _purecall definition is not provided in efi compilation, which leads to an
// 'undefined' error in asan builds.
extern "C" int _purecall(void) { return 0; }

namespace {
// Check for KVM or non-KVM QEMU using Hypervisor Vendor ID.
bool IsQemu() {
#if defined(__x86_64__)
  uint32_t eax;
  uint32_t name[3];
  __cpuid(0x40000000, eax, name[0], name[1], name[2]);
  std::string_view name_str(reinterpret_cast<const char*>(name), sizeof(name));
  return name_str == "TCGTCGTCGTCG"sv || name_str == "KVMKVMKVM\0\0\0"sv;
#elif defined(__aarch64__)
  uint64_t cpu_info;
  __asm__ volatile("mrs %0, MIDR_EL1" : "=r"(cpu_info));
  // Bits [31:24] define the implementor: 0x00 is "Reserved for software use".
  return (cpu_info & 0xFF000000) == 0x0;
#else
  return false;
#endif
}

// Always enable serial output if not already. Infra relies on serial to get device feedback.
// Without it, some CI/CQs fail.
void SetSerial() {
  PhysConsole& console = PhysConsole::Get();
  if (*console.serial() != FILE()) {
    return;
  }

  // QEMU routes serial output to the console. To avoid double printing, avoid using serial
  // output when running in QEMU.
  if (IsQemu()) {
    return;
  }

  // Temporarily set `gEfiSystemTable->ConOut` to NULL to force serial console setup.
  // This won't affect the graphic set up done earlier.
  //
  // This function can be removed once SetEfiStdout() can correctly set up both graphics and
  // serial output.

  auto conn_out = gEfiSystemTable->ConOut;
  gEfiSystemTable->ConOut = nullptr;
  SetEfiStdout(gEfiSystemTable);
  gEfiSystemTable->ConOut = conn_out;
}

// TODO(b/285053546) 'BootByte' usage should be removed in favour of ABR Metadata
bool ResetRebootMode(gigaboot::RebootMode reboot_mode, const AbrOps& abr_ops) {
  if (reboot_mode != gigaboot::RebootMode::kNormal &&
      !SetRebootMode(gigaboot::RebootMode::kNormal)) {
    printf("Failed to reset reboot mode\n");
    return false;
  }

  return true;
}

// Loads the desired reboot mode.
//
// The priority order is:
// 1. UEFI commandline arguments
// 2. One-shot flags
// 3. Default to normal boot
//
// If any one-shot flags are used, they are also reset by this function.
gigaboot::RebootMode LoadRebootMode() {
  // Only allow reading raw commandline from disk for testing on QEMU.
  bool allow_disk_fallback = IsQemu();
  if (auto reboot_mode = gigaboot::GetCommandlineRebootMode(allow_disk_fallback);
      reboot_mode.has_value()) {
    return reboot_mode.value();
  }

  // Print OneShotFlags from ABR
  AbrDataOneShotFlags one_shot_flags;
  ZirconBootOps zb_ops = gigaboot::GetZirconBootOps();
  AbrOps abr_ops = GetAbrOpsFromZirconBootOps(&zb_ops);
  AbrResult abr_res = AbrGetAndClearOneShotFlags(&abr_ops, &one_shot_flags);
  if (abr_res != kAbrResultOk) {
    printf("Warning: failed to get one shot flags from ABR; booting normally\n");
    return gigaboot::RebootMode::kNormal;
  }
  printf("abr.one_shot_flags = 0x%02x\n", one_shot_flags);

  gigaboot::RebootMode reboot_mode =
      gigaboot::GetOneShotRebootMode(one_shot_flags).value_or(gigaboot::RebootMode::kNormal);

  // TODO(b/285053546) 'BootByte' usage should be removed in favour of ABR Metadata
  // Reset previous reboot mode immediately to prevent it from being sticky.
  if (!ResetRebootMode(reboot_mode, abr_ops)) {
    printf("Error: failed to reset reboot mode\n");
    return gigaboot::RebootMode::kNormal;
  }

  return reboot_mode;
}

bool CheckFastbootKey() {
  constexpr zx::duration timeout = zx::sec(2);
  gigaboot::InputReceiver receiver(gEfiSystemTable);
  printf("Press f to enter fastboot.\n");
  std::optional<char> key = receiver.GetKeyPrompt("f", timeout, "Auto boot in");
  return key == 'f';
}

}  // namespace

// Main routine for gigaboot.
int gigaboot_main(int argc, char** argv) {
  printf("Gigaboot main\n");

  auto is_secureboot_on = gigaboot::IsSecureBootOn();
  if (is_secureboot_on.is_error()) {
    printf("Failed to query SecureBoot variable\n");
  } else {
    printf("Secure Boot: %s\n", *is_secureboot_on ? "On" : "Off");
  }

  // TODO(b/235489025): We reuse some legacy C gigaboot code for stuff like network stack.
  // This initializes the global variables the legacy code needs. Once these needed features are
  // re-implemented, remove these dependencies.
  xefi_init(gEfiImageHandle, gEfiSystemTable);

  // Log TPM info if the device has one.
  if (efi_status res = gigaboot::PrintTpm2Capability(); res != EFI_SUCCESS) {
    printf("Failed to log TPM 2.0 capability %s. TPM 2.0 may not be supported\n",
           gigaboot::EfiStatusToString(res));
  }

  gigaboot::RebootMode reboot_mode = LoadRebootMode();

  bool enter_fastboot = reboot_mode == gigaboot::RebootMode::kBootloader;
  if (enter_fastboot) {
    printf(
        "Your BIOS or ABR instructed Gigaboot to enter fastboot directly and skip normal boot.\n");
  } else {
    enter_fastboot = CheckFastbootKey();
  }

  if (enter_fastboot) {
    zx::result<> ret = gigaboot::FastbootTcpMain();
    if (ret.is_error()) {
      printf("Fastboot failed\n");
      return 1;
    }
  }

  ZirconBootFlags boot_flags = reboot_mode == gigaboot::RebootMode::kRecovery
                                   ? kZirconBootFlagsForceRecovery
                                   : kZirconBootFlagsNone;

  // TODO(b/236039205): Implement logic to construct these arguments for the API. This
  // is currently a placeholder for testing compilation/linking.
  ZirconBootOps zircon_boot_ops = gigaboot::GetZirconBootOps();
  ZirconBootResult res = LoadAndBoot(&zircon_boot_ops, boot_flags);
  if (res != kBootResultOK) {
    printf("Failed to boot zircon\n");
    return 1;
  }

  return 0;
}

// Main routine for booting embedded GBL.
int gbl_main(bool is_installer) {
  printf("Preparing to boot Generic Bootloader(GBL)...\n");
  if (is_installer) {
    printf("Image is for bootstrap/installer use. Always stops in Fastboot...\n");
  }
  auto mode = gigaboot::GetCommandlineRebootMode(IsQemu()).value_or(gigaboot::RebootMode::kNormal);
  bool stop_in_fastboot =
      is_installer || mode == gigaboot::RebootMode::kBootloader || CheckFastbootKey();
  if (gigaboot::LaunchGbl(stop_in_fastboot).is_error()) {
    printf("Failed to boot GBL\n");
  }
  return 1;
}

#ifndef GBL_STOP_IN_FASTBOOT
#define GBL_STOP_IN_FASTBOOT 0
#endif

int main(int argc, char** argv) {
  SetSerial();
#ifdef GIGABOOT_BOOT_GBL
  return gbl_main(GBL_STOP_IN_FASTBOOT);
#else
  return gigaboot_main(argc, argv);
#endif
}
