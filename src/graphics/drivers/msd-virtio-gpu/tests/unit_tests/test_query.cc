// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-virtio-gpu/include/magma-virtio-gpu-defs.h"
#include "src/graphics/drivers/msd-virtio-gpu/src/msd_virtio_device.h"
#include "src/graphics/lib/virtio/virtio-abi.h"

class VirtioGpuControlTest : public VirtioGpuControl {
 public:
  uint64_t GetCapabilitySetLimit() override { return 1; }

  zx::result<> SendHardwareCommand(cpp20::span<uint8_t> request,
                                   std::function<void(cpp20::span<uint8_t>)> callback) override {
    virtio_abi::ControlHeader* header =
        reinterpret_cast<virtio_abi::ControlHeader*>(request.data());
    static int constexpr kCapabilitySetInfoSize = 10;  // some arbitrary size < 4096

    if (header->type == virtio_abi::ControlType::kGetCapabilitySetInfoCommand) {
      virtio_abi::GetCapsetInfoResponse response = {
          .header = {.type = virtio_abi::ControlType::kCapabilitySetInfoResponse},
          .capset_id = 0,
          .capset_max_version = 0,
          .capset_max_size = kCapabilitySetInfoSize,
      };
      callback(cpp20::span<uint8_t>(reinterpret_cast<uint8_t*>(&response), sizeof(response)));
      return zx::ok();
    } else if (header->type == virtio_abi::ControlType::kGetCapabilitySetCommand) {
      std::vector<uint8_t> response(kCapabilitySetInfoSize + sizeof(virtio_abi::ControlHeader));
      *reinterpret_cast<virtio_abi::GetCapsetResponse*>(response.data()) = {
          .header = {.type = virtio_abi::ControlType::kCapabilitySetResponse}};
      callback(cpp20::span<uint8_t>(response));
      return zx::ok();
    }
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
};

TEST(TestQuery, CapabilitySet) {
  VirtioGpuControlTest control;
  MsdVirtioDevice device(&control);

  zx::vmo buffer;
  uint64_t* simple_result_ptr = nullptr;
  EXPECT_EQ(MAGMA_STATUS_OK,
            device.MsdQuery(kMagmaVirtioGpuQueryCapset, &buffer, simple_result_ptr));
}
