// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Provides a shared definition for the implementation of
// RegisterState by architecture.

#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TLS_STORAGE_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TLS_STORAGE_H_

#include <unistd.h>

#include <cinttypes>

namespace restricted_machine {

// TlsStorage represents the storage used for Thread Local Storage (TLS) ABIs.
// The layout of this struct is architecture-specific.
#if defined(__aarch64__)
struct TlsStorage {
  uint64_t tpidr;
};
#elif defined(__riscv)
struct TlsStorage {
  uint64_t tp;
};
#elif defined(__x86_64__)
struct TlsStorage {
  uint64_t fs_val;
  uint64_t gs_val;
};
#else
#error "Unsupported architecture targeted."
#endif

}  // namespace restricted_machine

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TLS_STORAGE_H_
