// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_TESTS_HYPERVISOR_CONSTANTS_H_
#define SRC_VIRTUALIZATION_TESTS_HYPERVISOR_CONSTANTS_H_

#include <zircon/limits.h>

#if __aarch64__
#include "arch/arm64/constants.h"
#elif __x86_64__
#include "arch/x64/constants.h"
#else
#error Unknown architecture.
#endif

#define VMO_SIZE 0x1000000
#define TRAP_PORT 0x11
#define TRAP_ADDR (VMO_SIZE - 2 * ZX_MAX_PAGE_SIZE)

// Trap address to indicate test success
#define EXIT_TEST_ADDR (VMO_SIZE - ZX_MAX_PAGE_SIZE)

// Trap address to indicate test failure
#define EXIT_TEST_FAILURE_ADDR (VMO_SIZE - ZX_MAX_PAGE_SIZE + 8)

#endif  // SRC_VIRTUALIZATION_TESTS_HYPERVISOR_CONSTANTS_H_
