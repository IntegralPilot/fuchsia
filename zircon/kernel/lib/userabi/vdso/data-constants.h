// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_USERABI_VDSO_DATA_CONSTANTS_H_
#define ZIRCON_KERNEL_LIB_USERABI_VDSO_DATA_CONSTANTS_H_

// This defines the struct shared with the kernel.
#include <lib/userabi/vdso-constants.h>

// References the DATA_CONSTANTS variable defined in data.S.
extern __LOCAL const struct vdso_constants DATA_CONSTANTS;

#endif  // ZIRCON_KERNEL_LIB_USERABI_VDSO_DATA_CONSTANTS_H_
