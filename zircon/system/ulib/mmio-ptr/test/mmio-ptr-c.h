// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_MMIO_PTR_TEST_MMIO_PTR_C_H_
#define ZIRCON_SYSTEM_ULIB_MMIO_PTR_TEST_MMIO_PTR_C_H_

#include <lib/mmio-ptr/mmio-ptr.h>

#if defined(__cplusplus)
extern "C" {
#endif

uint8_t c_MmioRead8(MMIO_PTR const volatile uint8_t* buffer);
uint16_t c_MmioRead16(MMIO_PTR const volatile uint16_t* buffer);
uint32_t c_MmioRead32(MMIO_PTR const volatile uint32_t* buffer);
uint64_t c_MmioRead64(MMIO_PTR const volatile uint64_t* buffer);

void c_MmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer);
void c_MmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer);
void c_MmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer);
void c_MmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer);

#if defined(__cplusplus)
}
#endif

#endif  // ZIRCON_SYSTEM_ULIB_MMIO_PTR_TEST_MMIO_PTR_C_H_
