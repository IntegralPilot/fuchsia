// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bootbyte.h"

#include <lib/zbi-format/internal/efi.h>
#include <stdint.h>
#include <zircon/compiler.h>

#include <efi/runtime-services.h>

char16_t kBootbyteVariableName[] = ZIRCON_BOOTBYTE_EFIVAR;
efi_guid kZirconVendorGuid = ZIRCON_VENDOR_GUID;

// FIXME(https://fxbug.dev/42078767): UBSan is detecting a function type mismatch between
// this and EfiBootbyOwner::GetVariable in fastboot_test.cc. I'm not exactly
// sure where the mismatch occurs since both functions have the same attribute,
// return type, and argument types. But for now, let's suppress the warning to
// allow the clang roll to proceed.
__attribute__((no_sanitize("function"))) efi_status get_bootbyte(efi_runtime_services* runtime,
                                                                 uint8_t* bootbyte) {
  size_t size = sizeof(*bootbyte);
  efi_status status = runtime->GetVariable((char16_t*)kBootbyteVariableName, &kZirconVendorGuid,
                                           NULL, &size, bootbyte);
  if (size != sizeof(*bootbyte)) {
    return EFI_BUFFER_TOO_SMALL;
  }
  return status;
}

// FIXME(https://fxbug.dev/42078767): UBSan is detecting a function type mismatch between
// this and EfiBootbyOwner::SetVariable in fastboot_test.cc. I'm not exactly
// sure where the mismatch occurs since both functions have the same attribute,
// return type, and argument types. But for now, let's suppress the warning to
// allow the clang roll to proceed.
__attribute__((no_sanitize("function"))) efi_status set_bootbyte(efi_runtime_services* runtime,
                                                                 uint8_t bootbyte) {
  return runtime->SetVariable((char16_t*)kBootbyteVariableName, &kZirconVendorGuid,
                              ZIRCON_BOOTBYTE_EFIATTR, sizeof(bootbyte), &bootbyte);
}
