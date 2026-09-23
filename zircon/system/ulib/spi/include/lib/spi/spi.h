// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SPI_SPI_H_
#define LIB_SPI_SPI_H_

#include <fidl/fuchsia.hardware.spi/cpp/wire.h>

zx_status_t spilib_transmit(fidl::UnownedClientEnd<fuchsia_hardware_spi::Device> device, void* data,
                            size_t length);

zx_status_t spilib_receive(fidl::UnownedClientEnd<fuchsia_hardware_spi::Device> device, void* data,
                           uint32_t length);

zx_status_t spilib_exchange(fidl::UnownedClientEnd<fuchsia_hardware_spi::Device> device,
                            void* txdata, void* rxdata, size_t length);

#endif  // LIB_SPI_SPI_H_
