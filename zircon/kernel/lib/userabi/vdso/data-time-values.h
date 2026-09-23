// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_USERABI_VDSO_DATA_TIME_VALUES_H_
#define ZIRCON_KERNEL_LIB_USERABI_VDSO_DATA_TIME_VALUES_H_

// This defines the struct used by libfasttime and the time syscalls.
#include <lib/fasttime/internal/abi.h>

// References the DATA_TIME_VALUES variable declared in data.S.
extern __LOCAL const fasttime::internal::TimeValues DATA_TIME_VALUES;

#endif  // ZIRCON_KERNEL_LIB_USERABI_VDSO_DATA_TIME_VALUES_H_
