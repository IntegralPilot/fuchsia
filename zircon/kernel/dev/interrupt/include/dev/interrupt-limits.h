// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_INTERRUPT_INCLUDE_DEV_INTERRUPT_LIMITS_H_
#define ZIRCON_KERNEL_DEV_INTERRUPT_INCLUDE_DEV_INTERRUPT_LIMITS_H_

#include <stdint.h>

#ifdef EXPERIMENTAL_APPLE
// T8122 has 1600 external IRQs. Vector zero is invalid; 1 and 2 are timer FIQs.
constexpr uint32_t MAX_INTERRUPTS = 1603;
#else
constexpr uint32_t MAX_INTERRUPTS = 1024;
#endif

#endif  // ZIRCON_KERNEL_DEV_INTERRUPT_INCLUDE_DEV_INTERRUPT_LIMITS_H_
