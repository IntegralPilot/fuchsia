// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/mmio-ptr/mmio-ptr.h>

extern "C" {

uint8_t mmio_read8(MMIO_PTR const volatile uint8_t* buffer);
void mmio_write8(uint8_t data, MMIO_PTR volatile uint8_t* buffer);

uint16_t mmio_read16(MMIO_PTR const volatile uint16_t* buffer);
void mmio_write16(uint16_t data, MMIO_PTR volatile uint16_t* buffer);

uint32_t mmio_read32(MMIO_PTR const volatile uint32_t* buffer);
void mmio_write32(uint32_t data, MMIO_PTR volatile uint32_t* buffer);

#ifdef _LP64
uint64_t mmio_read64(MMIO_PTR const volatile uint64_t* buffer);
void mmio_write64(uint64_t data, MMIO_PTR volatile uint64_t* buffer);
#endif

uint8_t mmio_read8(MMIO_PTR const volatile uint8_t* buffer) { return MmioRead8(buffer); }

void mmio_write8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) { MmioWrite8(data, buffer); }

uint16_t mmio_read16(MMIO_PTR const volatile uint16_t* buffer) { return MmioRead16(buffer); }

void mmio_write16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) { MmioWrite16(data, buffer); }

uint32_t mmio_read32(MMIO_PTR const volatile uint32_t* buffer) { return MmioRead32(buffer); }

void mmio_write32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) { MmioWrite32(data, buffer); }

#ifdef _LP64
uint64_t mmio_read64(MMIO_PTR const volatile uint64_t* buffer) { return MmioRead64(buffer); }

void mmio_write64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) { MmioWrite64(data, buffer); }
#endif

}  // extern "C"
