// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "include/lib/spi/spi.h"

#include <fidl/fuchsia.hardware.spi/cpp/wire.h>

zx_status_t spilib_transmit(fidl::UnownedClientEnd<fuchsia_hardware_spi::Device> device, void* data,
                            size_t length) {
  const fidl::WireResult result = fidl::WireCall(device)->TransmitVector(
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(data), length));
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  return response.status;
}

zx_status_t spilib_receive(fidl::UnownedClientEnd<fuchsia_hardware_spi::Device> device, void* data,
                           uint32_t length) {
  const fidl::WireResult result = fidl::WireCall(device)->ReceiveVector(length);
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return status;
  }
  if (response.data.size() > length) {
    return ZX_ERR_IO_OVERRUN;
  }
  memcpy(data, response.data.data(), response.data.size());
  return ZX_OK;
}

zx_status_t spilib_exchange(fidl::UnownedClientEnd<fuchsia_hardware_spi::Device> device,
                            void* txdata, void* rxdata, size_t length) {
  const fidl::WireResult result = fidl::WireCall(device)->ExchangeVector(
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(txdata), length));
  if (result.status() != ZX_OK) {
    return result.status();
  }
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return status;
  }
  if (response.rxdata.size() > length) {
    return ZX_ERR_IO_OVERRUN;
  }
  memcpy(rxdata, response.rxdata.data(), response.rxdata.size());
  return ZX_OK;
}
