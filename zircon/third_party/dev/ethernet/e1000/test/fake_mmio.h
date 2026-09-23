// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_TEST_FAKE_MMIO_H_
#define ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_TEST_FAKE_MMIO_H_

#include <lib/mmio-ptr/mmio-ptr.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#ifdef __cplusplus

namespace e1000::test {

class MmioInterceptor {
 public:
  virtual ~MmioInterceptor();

  virtual void OnMmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) {
    *(uint8_t*)buffer = data;
  }
  virtual void OnMmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) {
    *(uint16_t*)buffer = data;
  }
  virtual void OnMmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) {
    *(uint32_t*)buffer = data;
  }
  virtual uint8_t OnMmioRead8(MMIO_PTR const volatile uint8_t* buffer) { return *(uint8_t*)buffer; }
  virtual uint16_t OnMmioRead16(MMIO_PTR const volatile uint16_t* buffer) {
    return *(uint16_t*)buffer;
  }
  virtual uint32_t OnMmioRead32(MMIO_PTR const volatile uint32_t* buffer) {
    return *(uint32_t*)buffer;
  }
  virtual void OnMmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) {
    *(uint64_t*)buffer = data;
  }
  virtual uint64_t OnMmioRead64(MMIO_PTR const volatile uint64_t* buffer) {
    return *(uint64_t*)buffer;
  }
};

void SetMmioInterceptor(MmioInterceptor* interceptor);

}  // namespace e1000::test

#endif  // __cplusplus

__BEGIN_CDECLS

void TestMmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer);
void TestMmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer);
void TestMmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer);
uint8_t TestMmioRead8(MMIO_PTR const volatile uint8_t* buffer);
uint16_t TestMmioRead16(MMIO_PTR const volatile uint16_t* buffer);
uint32_t TestMmioRead32(MMIO_PTR const volatile uint32_t* buffer);
void TestMmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer);
uint64_t TestMmioRead64(MMIO_PTR const volatile uint64_t* buffer);

#define MmioWrite8(data, buffer) TestMmioWrite8(data, buffer)
#define MmioWrite16(data, buffer) TestMmioWrite16(data, buffer)
#define MmioWrite32(data, buffer) TestMmioWrite32(data, buffer)
#define MmioRead8(buffer) TestMmioRead8(buffer)
#define MmioRead16(buffer) TestMmioRead16(buffer)
#define MmioRead32(buffer) TestMmioRead32(buffer)
#define MmioWrite64(data, buffer) TestMmioWrite64(data, buffer)
#define MmioRead64(buffer) TestMmioRead64(buffer)

__END_CDECLS

#endif  // ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_TEST_FAKE_MMIO_H_
