// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-gpu.h"

#include <fidl/fuchsia.hardware.gpu.amlogic/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/vmo.h>

#include <gtest/gtest.h>
#include <soc/aml-common/aml-registers.h>

#include "s905d2-gpu.h"
#include "src/devices/registers/testing/mock-registers/mock-registers.h"

namespace aml_gpu {

class TestAmlGpu {
 public:
  void TestSetClkFreq() {
    aml_gpu_.gpu_block_ = &s905d2_gpu_blocks;
    zx::vmo vmo;
    constexpr uint32_t kHiuRegisterSize = 1024 * 16;
    ASSERT_EQ(ZX_OK, zx::vmo::create(kHiuRegisterSize, 0, &vmo));
    zx::result<fdf::MmioBuffer> result =
        fdf::MmioBuffer::Create(0, kHiuRegisterSize, std::move(vmo), ZX_CACHE_POLICY_CACHED);
    ASSERT_TRUE(result.is_ok());
    aml_gpu_.hiu_buffer_ = std::move(result.value());

    aml_gpu_.SetClkFreqSource(1);
    uint32_t value = aml_gpu_.hiu_buffer_->Read32(0x6c << 2);
    // Mux should be set to 1.
    EXPECT_EQ(1u, value >> kFinalMuxBitShift);
    uint32_t parent_mux_value = (value >> 16) & 0xfff;
    uint32_t source = parent_mux_value >> 9;
    bool enabled = (parent_mux_value >> kClkEnabledBitShift) & 1;
    uint32_t divisor = (parent_mux_value & 0xff) + 1;
    EXPECT_EQ(S905D2_FCLK_DIV5, source);
    EXPECT_TRUE(enabled);
    EXPECT_EQ(1u, divisor);
  }

  void TestInitialClkFreq() {
    aml_gpu_.gpu_block_ = &s905d2_gpu_blocks;
    zx::vmo vmo;
    constexpr uint32_t kHiuRegisterSize = 1024 * 16;
    ASSERT_EQ(ZX_OK, zx::vmo::create(kHiuRegisterSize, 0, &vmo));
    zx::result<fdf::MmioBuffer> hiu_result =
        fdf::MmioBuffer::Create(0, kHiuRegisterSize, std::move(vmo), ZX_CACHE_POLICY_CACHED);
    ASSERT_TRUE(hiu_result.is_ok());
    aml_gpu_.hiu_buffer_ = std::move(hiu_result.value());
    ASSERT_EQ(ZX_OK, zx::vmo::create(kHiuRegisterSize, 0, &vmo));
    zx::result<fdf::MmioBuffer> gpu_result =
        fdf::MmioBuffer::Create(0, kHiuRegisterSize, std::move(vmo), ZX_CACHE_POLICY_CACHED);
    ASSERT_TRUE(gpu_result.is_ok());
    aml_gpu_.gpu_buffer_ = std::move(gpu_result.value());
    async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
    loop.StartThread();
    mock_registers::MockRegisters reset_mock(loop.dispatcher());
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_registers::Device>();
    reset_mock.Init(std::move(endpoints->server));
    aml_gpu_.reset_register_ = fidl::WireSyncClient(std::move(endpoints->client));
    reset_mock.ExpectWrite<uint32_t>(aml_gpu_.gpu_block_->reset0_mask_offset,
                                     aml_registers::MALI_RESET0_MASK, 0);
    reset_mock.ExpectWrite<uint32_t>(aml_gpu_.gpu_block_->reset0_level_offset,
                                     aml_registers::MALI_RESET0_MASK, 0);
    reset_mock.ExpectWrite<uint32_t>(aml_gpu_.gpu_block_->reset2_mask_offset,
                                     aml_registers::MALI_RESET2_MASK, 0);
    reset_mock.ExpectWrite<uint32_t>(aml_gpu_.gpu_block_->reset2_level_offset,
                                     aml_registers::MALI_RESET2_MASK, 0);
    reset_mock.ExpectWrite<uint32_t>(aml_gpu_.gpu_block_->reset0_level_offset,
                                     aml_registers::MALI_RESET0_MASK,
                                     aml_registers::MALI_RESET0_MASK);
    reset_mock.ExpectWrite<uint32_t>(aml_gpu_.gpu_block_->reset2_level_offset,
                                     aml_registers::MALI_RESET2_MASK,
                                     aml_registers::MALI_RESET2_MASK);
    aml_gpu_.gp0_init_succeeded_ = true;
    aml_gpu_.InitClock();
    uint32_t value = aml_gpu_.hiu_buffer_->Read32(0x6c << 2);
    // Glitch-free mux should stay unchanged.
    EXPECT_EQ(0u, value >> kFinalMuxBitShift);
    uint32_t parent_mux_value = value & 0xfff;
    uint32_t source = parent_mux_value >> 9;
    bool enabled = (parent_mux_value >> kClkEnabledBitShift) & 1;
    uint32_t divisor = (parent_mux_value & 0xff) + 1;
    // S905D2 starts at the highest frequency by default.
    EXPECT_EQ(S905D2_GP0, source);
    EXPECT_TRUE(enabled);
    EXPECT_EQ(1u, divisor);
    EXPECT_EQ(ZX_OK, reset_mock.VerifyAll());
  }

  fdf_testing::ScopedGlobalLogger logger_;

  aml_gpu::AmlGpu aml_gpu_;
};
}  // namespace aml_gpu

TEST(AmlGpu, SetClkFreq) { aml_gpu::TestAmlGpu().TestSetClkFreq(); }

TEST(AmlGpu, InitialClkFreq) { aml_gpu::TestAmlGpu().TestInitialClkFreq(); }
