// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef __GBL_EFI_PROTOCOLS_H__
#define __GBL_EFI_PROTOCOLS_H__

#include <efi/types.h>

extern bool g_should_stop_in_fastboot;

namespace gigaboot {

efi_status InstallGblEfiBootControlProtocol();
efi_status InstallGblEfiFastbootProtocol();

}  // namespace gigaboot

#endif  // __GBL_EFI_PROTOCOLS_H__
