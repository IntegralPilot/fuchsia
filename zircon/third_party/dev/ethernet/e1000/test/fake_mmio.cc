// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "zircon/third_party/dev/ethernet/e1000/test/fake_mmio.h"

namespace e1000::test {

MmioInterceptor* g_mmio_interceptor = nullptr;

MmioInterceptor::~MmioInterceptor() = default;

void SetMmioInterceptor(MmioInterceptor* interceptor) { g_mmio_interceptor = interceptor; }

}  // namespace e1000::test

__BEGIN_CDECLS

void TestMmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  e1000::test::g_mmio_interceptor->OnMmioWrite8(data, buffer);
}

void TestMmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  e1000::test::g_mmio_interceptor->OnMmioWrite16(data, buffer);
}

void TestMmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  e1000::test::g_mmio_interceptor->OnMmioWrite32(data, buffer);
}

uint8_t TestMmioRead8(MMIO_PTR const volatile uint8_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  return e1000::test::g_mmio_interceptor->OnMmioRead8(buffer);
}

uint16_t TestMmioRead16(MMIO_PTR const volatile uint16_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  return e1000::test::g_mmio_interceptor->OnMmioRead16(buffer);
}

uint32_t TestMmioRead32(MMIO_PTR const volatile uint32_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  return e1000::test::g_mmio_interceptor->OnMmioRead32(buffer);
}

void TestMmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  e1000::test::g_mmio_interceptor->OnMmioWrite64(data, buffer);
}

uint64_t TestMmioRead64(MMIO_PTR const volatile uint64_t* buffer) {
  ZX_ASSERT_MSG(e1000::test::g_mmio_interceptor, "MMIO interceptor not set");
  return e1000::test::g_mmio_interceptor->OnMmioRead64(buffer);
}

__END_CDECLS
