// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/fdio/directory.h>
#include <lib/mmio-ptr/fake.h>

#include <memory>
#include <optional>

#include <gtest/gtest.h>

#include "amlogic-video.h"
#include "src/media/drivers/amlogic_decoder/decoder_core.h"
#include "vdec1.h"

namespace amlogic_decoder {
namespace test {
namespace {

fidl::SyncClient<fuchsia_sysmem2::Allocator> ConnectToSysmem() {
  auto connect_result = component::Connect<fuchsia_sysmem2::Allocator>();
  ZX_ASSERT(connect_result.is_ok());
  return fidl::SyncClient(std::move(*connect_result));
}

class FakeOwner : public DecoderCore::Owner {
 public:
  explicit FakeOwner(MmioRegisters* mmio) : mmio_(mmio) {
    zx::result bti = fake_bti::CreateFakeBti();
    EXPECT_EQ(ZX_OK, bti.status_value());
    bti_ = std::move(bti.value());
    allocator_ = ConnectToSysmem();
  }

  void set_device_type(DeviceType type) { device_type_ = type; }

  MmioRegisters* mmio() override { return mmio_; }
  zx_status_t UngateClocks() override {
    clocks_gated_ = false;
    return ZX_OK;
  }
  zx_status_t GateClocks() override {
    clocks_gated_ = true;
    return ZX_OK;
  }
  zx::unowned_bti bti() override { return zx::unowned_bti(bti_); }
  DeviceType device_type() override { return device_type_; }
  fidl::SyncClient<fuchsia_sysmem2::Allocator>& SysmemAllocatorSync() override {
    return allocator_;
  }
  zx_status_t ToggleClock(ClockType type, bool enable) override {
    enable_clock_state_[static_cast<int>(type)] = enable;
    return ZX_OK;
  }
  bool enable_clock_state(ClockType type) const {
    return enable_clock_state_[static_cast<int>(type)];
  }

  bool clocks_gated() const { return clocks_gated_; }

 private:
  zx::bti bti_;
  bool clocks_gated_ = true;
  fidl::SyncClient<fuchsia_sysmem2::Allocator> allocator_;
  MmioRegisters* mmio_;
  std::array<bool, static_cast<int>(ClockType::kMax)> enable_clock_state_{};
  DeviceType device_type_ = DeviceType::kG12B;
};

constexpr uint32_t kDosbusMemorySize = 0x10000;
constexpr uint32_t kAobusMemorySize = 0x10000;
constexpr uint32_t kDmcMemorySize = 0x10000;
constexpr uint32_t kHiuBusMemorySize = 0x10000;
}  // namespace

class Vdec1UnitTest : public testing::Test {
 public:
  void SetUp() override {
    dosbus_ = fdf_testing::CreateMmioBuffer(kDosbusMemorySize, ZX_CACHE_POLICY_UNCACHED);
    aobus_ = fdf_testing::CreateMmioBuffer(kAobusMemorySize, ZX_CACHE_POLICY_UNCACHED);
    dmc_ = fdf_testing::CreateMmioBuffer(kDmcMemorySize, ZX_CACHE_POLICY_UNCACHED);
    hiubus_ = fdf_testing::CreateMmioBuffer(kHiuBusMemorySize, ZX_CACHE_POLICY_UNCACHED);

    mmio_ = std::unique_ptr<MmioRegisters>(
        new MmioRegisters{&*dosbus_, &*aobus_, &*dmc_, &*hiubus_, /*reset*/ nullptr,
                          /*parser*/ nullptr, /*demux*/ nullptr});
  }

 protected:
  std::optional<DosRegisterIo> dosbus_;
  std::optional<AoRegisterIo> aobus_;
  std::optional<DmcRegisterIo> dmc_;
  std::optional<HiuRegisterIo> hiubus_;

  std::unique_ptr<MmioRegisters> mmio_;
};

TEST_F(Vdec1UnitTest, PowerOn) {
  FakeOwner fake_owner(mmio_.get());
  auto decoder = std::make_unique<Vdec1>(&fake_owner);

  HhiVdecClkCntl::Get().FromValue(0xffff0000).WriteTo(fake_owner.mmio()->hiubus);
  DosGclkEn::Get().FromValue(0xfffffc00).WriteTo(fake_owner.mmio()->dosbus);
  {  // scope power_ref
    PowerReference power_ref(decoder.get());
    // confirm non vdec bits weren't touched
    EXPECT_EQ(0xffff0000,
              HhiVdecClkCntl::Get().ReadFrom(fake_owner.mmio()->hiubus).reg_value() & 0xffff0000);
    EXPECT_EQ(0xfffffc00, DosGclkEn::Get().ReadFrom(fake_owner.mmio()->dosbus).reg_value());
    EXPECT_TRUE(fake_owner.enable_clock_state(ClockType::kGclkVdec));
    EXPECT_FALSE(fake_owner.clocks_gated());
  }  // ~power_ref
  EXPECT_TRUE(fake_owner.clocks_gated());
}

TEST_F(Vdec1UnitTest, PowerOnSm1) {
  FakeOwner fake_owner(mmio_.get());

  fake_owner.set_device_type(DeviceType::kSM1);
  auto decoder = std::make_unique<Vdec1>(&fake_owner);

  AoRtiGenPwrIso0::Get().FromValue(0xffffffff).WriteTo(fake_owner.mmio()->aobus);
  AoRtiGenPwrSleep0::Get().FromValue(0xffffffff).WriteTo(fake_owner.mmio()->aobus);
  HhiVdecClkCntl::Get().FromValue(0xffff0000).WriteTo(fake_owner.mmio()->hiubus);
  DosGclkEn::Get().FromValue(0xfffffc00).WriteTo(fake_owner.mmio()->dosbus);
  {  // scope power_ref
    PowerReference power_ref(decoder.get());
    // confirm non vdec bits weren't touched
    EXPECT_EQ(0xffff0000,
              HhiVdecClkCntl::Get().ReadFrom(fake_owner.mmio()->hiubus).reg_value() & 0xffff0000);
    EXPECT_EQ(0xfffffc00, DosGclkEn::Get().ReadFrom(fake_owner.mmio()->dosbus).reg_value());
    EXPECT_EQ(0xffffffffu & ~2,
              AoRtiGenPwrIso0::Get().ReadFrom(fake_owner.mmio()->aobus).reg_value());
    EXPECT_EQ(0xffffffffu & ~2,
              AoRtiGenPwrSleep0::Get().ReadFrom(fake_owner.mmio()->aobus).reg_value());

    EXPECT_TRUE(fake_owner.enable_clock_state(ClockType::kGclkVdec));
    EXPECT_FALSE(fake_owner.clocks_gated());
  }  // ~power_ref

  EXPECT_TRUE(fake_owner.clocks_gated());
  EXPECT_EQ(0xffffffffu, AoRtiGenPwrIso0::Get().ReadFrom(fake_owner.mmio()->aobus).reg_value());
  EXPECT_EQ(0xffffffffu, AoRtiGenPwrSleep0::Get().ReadFrom(fake_owner.mmio()->aobus).reg_value());
}

}  // namespace test
}  // namespace amlogic_decoder
