// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AML_CANVAS_BOARD_RESOURCES_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AML_CANVAS_BOARD_RESOURCES_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/bti.h>

namespace aml_canvas {

// The *ResourceIndex scoped enums define the interface between the board driver
// and the aml-canvas driver.

// The resource ordering in the board driver's `canvas_mmios` table.
enum class MmioResourceIndex : uint8_t {
  kDmc = 0,  // DMC (DDR memory controller)
};

// The resource ordering in the board driver's `canvas_btis` table.
enum class BtiResourceIndex : uint8_t {
  kCanvas = 0,  // BTI used for canvas transfers.
};

// Typesafe wrapper for [`fuchsia.hardware.platform.device/Device.GetMmioById`].
//
// If the result is successful, the MmioBuffer is guaranteed to be valid.
zx::result<fdf::MmioBuffer> MapMmio(
    MmioResourceIndex mmio_index,
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> pdev);

// Typesafe wrapper for [`fuchsia.hardware.platform.device/Device.GetBtiById`].
//
// If the result is successful, the zx::bti is guaranteed to be valid.
zx::result<zx::bti> GetBti(BtiResourceIndex bti_index,
                           fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> pdev);

}  // namespace aml_canvas

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AML_CANVAS_BOARD_RESOURCES_H_
