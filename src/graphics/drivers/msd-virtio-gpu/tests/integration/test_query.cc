// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/magma.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/vmo.h>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-virtio-gpu/include/magma-virtio-gpu-defs.h"
#include "src/graphics/lib/virtio/virtio-abi.h"

#define MAGMA_VENDOR_ID_VIRTIO 0x1af4

namespace {

class TestDevice : public magma::TestDeviceBase {
 public:
  TestDevice() : magma::TestDeviceBase(MAGMA_VENDOR_ID_VIRTIO) {}

  void TestCapset() {
    constexpr uint16_t kCapsetVersion = 0;
    constexpr uint64_t kQueryId =
        kMagmaVirtioGpuQueryCapset |
        (static_cast<uint64_t>(virtio_abi::CapsetId::kCapsetGfxstream) << 32) |
        (static_cast<uint64_t>(kCapsetVersion) << 48);

    magma_handle_t magma_buffer_handle = 0;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device(), kQueryId, &magma_buffer_handle, /*result_out=*/nullptr));
    EXPECT_NE(magma_buffer_handle, 0u);

    zx::vmo capset_vmo(magma_buffer_handle);

    uint64_t size = 0;
    EXPECT_EQ(ZX_OK, capset_vmo.get_size(&size));
    EXPECT_GE(size, 4096u);
  }
};

}  // namespace

TEST(Query, Capset) { TestDevice().TestCapset(); }
