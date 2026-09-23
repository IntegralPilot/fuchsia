// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/aml-canvas/aml-canvas.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mock-mmio/cpp/globally-ordered-region.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>

#include <vector>

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/aml-canvas/dmc-regs.h"
#include "src/lib/testing/predicates/status.h"

namespace aml_canvas {

namespace {

const uint32_t kVmoTestSize = zx_system_get_page_size();

constexpr fuchsia_hardware_amlogiccanvas::wire::CanvasInfo test_canvas_info = {
    .height = 240,
    .stride_bytes = 16,
    .blkmode = fuchsia_hardware_amlogiccanvas::CanvasBlockMode::kLinear,
};

constexpr fuchsia_hardware_amlogiccanvas::wire::CanvasInfo invalid_canvas_info = {
    .height = 240,
    .stride_bytes = 15,
    .blkmode = fuchsia_hardware_amlogiccanvas::CanvasBlockMode::kLinear,
};

// Register addresses from the A311D datasgeet section 13.1.2 "DDR" >
// "Register description". DMC is typo-ed as DC.
constexpr int kCanvasLutDataLowOffset = 0x0012 * 4;
constexpr int kCanvasLutDataHighOffset = 0x0013 * 4;
constexpr int kCanvasLutAddressOffset = 0x0014 * 4;

class AmlCanvasTest : public testing::Test {
 public:
  void SetUp() override {
    zx::result bti = fake_bti::CreateFakeBti();
    EXPECT_OK(bti);

    auto endpoints = fidl::Endpoints<fuchsia_hardware_amlogiccanvas::Device>::Create();

    // TODO(136015): This test should invoke ::Create(), which requires a FakePDevFidl.
    canvas_ = std::make_unique<AmlCanvas>(mmio_range_.GetMmioBuffer(), std::move(bti.value()),
                                          inspect::Inspector{});

    binding_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(endpoints.server),
                     canvas_.get(), fidl::kIgnoreBindingClosure);

    canvas_client_.Bind(std::move(endpoints.client));
  }

  void TearDown() override { mmio_range_.CheckAllAccessesReplayed(); }

  // Because this test is using a fidl::WireSyncClient, we need to run any ops on the client on
  // their own thread because the testing thread is shared with the client end.
  static void RunSyncClientTask(fit::closure task) {
    async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
    loop.StartThread();
    zx::result result = fdf::RunOnDispatcherSync(loop.dispatcher(), std::move(task));
    ASSERT_OK(result);
  }

  zx_status_t CreateNewCanvas() {
    zx::vmo vmo;
    EXPECT_OK(zx::vmo::create(kVmoTestSize, 0, &vmo));

    zx_status_t status{ZX_OK};
    RunSyncClientTask([&]() {
      fidl::WireResult result = canvas_client_->Config(std::move(vmo), 0, test_canvas_info);
      if (!result.ok()) {
        status = result.error().status();
      } else if (result->is_error()) {
        status = result->error_value();
      } else {
        canvas_indices_.push_back(result->value()->canvas_idx);
      }
    });
    return status;
  }

  zx_status_t CreateNewCanvasInvalid() {
    zx::vmo vmo;
    EXPECT_OK(zx::vmo::create(kVmoTestSize, 0, &vmo));

    zx_status_t status = ZX_OK;
    RunSyncClientTask([&]() {
      fidl::WireResult result = canvas_client_->Config(std::move(vmo), 0, invalid_canvas_info);
      if (!result.ok()) {
        status = result.error().status();
      } else if (result->is_error()) {
        status = result->error_value();
      }
    });

    // We should be returning an error since we are creating an invalid canvas.
    EXPECT_NE(ZX_OK, status);
    return status;
  }

  zx_status_t FreeCanvas(uint8_t index) {
    auto it = std::find(canvas_indices_.begin(), canvas_indices_.end(), index);
    if (it != canvas_indices_.end()) {
      canvas_indices_.erase(it);
    }

    zx_status_t status{ZX_OK};
    RunSyncClientTask([&]() {
      fidl::WireResult result = canvas_client_->Free(index);
      if (!result.ok()) {
        status = result.error().status();
      } else if (result->is_error()) {
        status = result->error_value();
      }
    });
    return status;
  }

  zx_status_t FreeAllCanvases() {
    zx_status_t status{ZX_OK};
    while (!canvas_indices_.empty()) {
      uint8_t index = canvas_indices_.back();
      canvas_indices_.pop_back();

      RunSyncClientTask([&]() {
        auto result = canvas_client_->Free(index);
        if (!result.ok()) {
          status = result.error().status();
        } else if (result->is_error()) {
          status = result->error_value();
        };
      });
      if (status != ZX_OK) {
        return status;
      }
    }
    return status;
  }

  void SetRegisterExpectations() {
    // TODO(costan): Remove the read expectations when we get rid of the unnecessary R/M/W.
    mmio_range_.Expect(mock_mmio::GloballyOrderedRegion::AccessList({
        {.address = kCanvasLutDataLowOffset, .value = CanvasLutDataLowValue(), .write = true},
        {.address = kCanvasLutDataHighOffset, .value = CanvasLutDataHighValue(), .write = true},
        {.address = kCanvasLutAddressOffset,
         .value = CanvasLutAddrValue(NextCanvasIndex()),
         .write = true},
        {.address = kCanvasLutDataHighOffset, .value = CanvasLutDataHighValue()},
    }));
  }

  void SetRegisterExpectations(uint8_t index) {
    // TODO(costan): Remove the read expectations when we get rid of the unnecessary R/M/W.
    mmio_range_.Expect(mock_mmio::GloballyOrderedRegion::AccessList({
        {.address = kCanvasLutDataLowOffset, .value = CanvasLutDataLowValue(), .write = true},
        {.address = kCanvasLutDataHighOffset, .value = CanvasLutDataHighValue(), .write = true},
        {.address = kCanvasLutAddressOffset, .value = CanvasLutAddrValue(index), .write = true},
        {.address = kCanvasLutDataHighOffset, .value = CanvasLutDataHighValue()},
    }));
  }

 private:
  uint8_t NextCanvasIndex() { return static_cast<uint8_t>(canvas_indices_.size()); }

  uint32_t CanvasLutDataLowValue() {
    auto data_low = CanvasLutDataLow::Get().FromValue(0);
    data_low.SetDmcCavWidth(test_canvas_info.stride_bytes >> 3);
    data_low.set_dmc_cav_addr(FAKE_BTI_PHYS_ADDR >> 3);
    return data_low.reg_value();
  }

  uint32_t CanvasLutDataHighValue() {
    auto data_high = CanvasLutDataHigh::Get().FromValue(0);
    data_high.SetDmcCavWidth(test_canvas_info.stride_bytes >> 3);
    data_high.set_dmc_cav_height(test_canvas_info.height);
    data_high.set_dmc_cav_blkmode(static_cast<uint32_t>(test_canvas_info.blkmode));
    data_high.set_dmc_cav_xwrap(
        test_canvas_info.flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrapHorizontal ? 1
                                                                                              : 0);
    data_high.set_dmc_cav_ywrap(
        test_canvas_info.flags & fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrapVertical ? 1
                                                                                            : 0);
    data_high.set_dmc_cav_endianness(static_cast<uint32_t>(test_canvas_info.endianness));
    return data_high.reg_value();
  }

  uint32_t CanvasLutAddrValue(uint8_t index) {
    auto lut_addr = CanvasLutAddr::Get().FromValue(0);
    lut_addr.set_dmc_cav_addr_index(index);
    lut_addr.set_dmc_cav_addr_wr(1);
    return lut_addr.reg_value();
  }

  fdf_testing::DriverRuntime runtime_;
  fdf_testing::ScopedGlobalLogger logger_;

  std::vector<uint8_t> canvas_indices_;

  static constexpr int kMmioRangeSize = 0x100;
  mock_mmio::GloballyOrderedRegion mmio_range_{kMmioRangeSize,
                                               mock_mmio::GloballyOrderedRegion::Size::k32};

  // Note, set up to execute on testing thread.
  std::unique_ptr<AmlCanvas> canvas_;

  std::optional<fidl::ServerBinding<fuchsia_hardware_amlogiccanvas::Device>> binding_;
  fidl::WireSyncClient<fuchsia_hardware_amlogiccanvas::Device> canvas_client_;
};

TEST_F(AmlCanvasTest, CanvasConfigFreeSingle) {
  SetRegisterExpectations();
  EXPECT_OK(CreateNewCanvas());

  EXPECT_OK(FreeAllCanvases());
}

TEST_F(AmlCanvasTest, CanvasConfigFreeMultipleSequential) {
  // Create 5 canvases in sequence and verify that their indices are 0 through 4.
  for (int i = 0; i < 5; i++) {
    SetRegisterExpectations();
    EXPECT_OK(CreateNewCanvas());
  }

  // Free all 5 canvases created above.
  EXPECT_OK(FreeAllCanvases());
}

TEST_F(AmlCanvasTest, CanvasConfigFreeMultipleInterleaved) {
  // Create 5 canvases in sequence.
  for (int i = 0; i < 5; i++) {
    SetRegisterExpectations();
    EXPECT_OK(CreateNewCanvas());
  }

  // Free canvas index 1, so the next one created has index 1.
  EXPECT_OK(FreeCanvas(1));

  SetRegisterExpectations(1);
  EXPECT_OK(CreateNewCanvas());

  // Free canvas index 3, so the next one created has index 3.
  EXPECT_OK(FreeCanvas(3));

  SetRegisterExpectations(3);
  EXPECT_OK(CreateNewCanvas());

  EXPECT_OK(FreeAllCanvases());
}

TEST_F(AmlCanvasTest, CanvasFreeInvalidIndex) {
  // Free a canvas without having created any.
  EXPECT_STATUS(FreeCanvas(0), ZX_ERR_INVALID_ARGS);
}

TEST_F(AmlCanvasTest, CanvasConfigMaxLimit) {
  // Create canvases until the look-up table is full.
  for (size_t i = 0; i < kNumCanvasEntries; i++) {
    SetRegisterExpectations();
    EXPECT_OK(CreateNewCanvas());
  }

  // Try to create another canvas, and verify that it fails.
  EXPECT_STATUS(CreateNewCanvas(), ZX_ERR_NOT_FOUND);

  EXPECT_OK(FreeAllCanvases());
}

TEST_F(AmlCanvasTest, CanvasConfigUnaligned) {
  // Try to create a canvas with unaligned fuchsia_hardware_amlogiccanvas::wire::CanvasInfo width,
  // and verify that it fails.
  EXPECT_STATUS(CreateNewCanvasInvalid(), ZX_ERR_INVALID_ARGS);
}

}  // namespace

}  //  namespace aml_canvas
