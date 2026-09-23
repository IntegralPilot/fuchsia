// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mmio-ptr-c.h"

// We compile this file as C to verify that the mmio-ptr.h header is
// compatible with the C compiler.

uint8_t c_MmioRead8(MMIO_PTR const volatile uint8_t* buffer) { return MmioRead8(buffer); }

uint16_t c_MmioRead16(MMIO_PTR const volatile uint16_t* buffer) { return MmioRead16(buffer); }

uint32_t c_MmioRead32(MMIO_PTR const volatile uint32_t* buffer) { return MmioRead32(buffer); }

uint64_t c_MmioRead64(MMIO_PTR const volatile uint64_t* buffer) { return MmioRead64(buffer); }

void c_MmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) { MmioWrite8(data, buffer); }

void c_MmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) { MmioWrite16(data, buffer); }

void c_MmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) { MmioWrite32(data, buffer); }

void c_MmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) { MmioWrite64(data, buffer); }
