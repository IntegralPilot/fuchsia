// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_ARCH_HELPERS_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_ARCH_HELPERS_H_

namespace restricted_machine {

namespace internal {
// These helpers are defined in the per-arch assembly support files.
extern "C" void load_fpu_registers(void* in);
extern "C" void store_fpu_registers(void* out);
}  // namespace internal

}  // namespace restricted_machine

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_ARCH_HELPERS_H_
