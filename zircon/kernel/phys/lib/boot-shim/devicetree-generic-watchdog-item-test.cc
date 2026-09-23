// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/devicetree/testing/loaded-dtb.h>
#include <lib/fit/defer.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/image.h>

#include <optional>

#include <zxtest/zxtest.h>

namespace {

using devicetree::testing::LoadDtb;
using devicetree::testing::LoadedDtb;

struct MmioHelper {
  static uint32_t Read(uint64_t base) { return value; }
  static void Write(uint64_t base, uint32_t val) { value = val; }

  static inline uint32_t value = 0;
};

class DevicetreeGenericWatchdogItemTest : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    auto loaded_dtb = LoadDtb("qcom_msm_watchdog.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    qcom_msm_watchdog_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("qcom_msm_watchdog_multiple_regs.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    qcom_msm_watchdog_multiuple_regs_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("empty.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    empty_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    empty_ = std::nullopt;
    qcom_msm_watchdog_ = std::nullopt;
    qcom_msm_watchdog_multiuple_regs_ = std::nullopt;
  }

  void SetUp() final { MmioHelper::value = 0; }

  auto qcom_msm_watchdog() { return qcom_msm_watchdog_->fdt(); }
  auto qcom_msm_watchdog_multiuple_regs() { return qcom_msm_watchdog_multiuple_regs_->fdt(); }
  auto empty() { return empty_->fdt(); }

 private:
  static std::optional<LoadedDtb> qcom_msm_watchdog_;
  static std::optional<LoadedDtb> qcom_msm_watchdog_multiuple_regs_;
  static std::optional<LoadedDtb> empty_;
};

std::optional<LoadedDtb> DevicetreeGenericWatchdogItemTest::empty_ = std::nullopt;
std::optional<LoadedDtb> DevicetreeGenericWatchdogItemTest::qcom_msm_watchdog_ = std::nullopt;
std::optional<LoadedDtb> DevicetreeGenericWatchdogItemTest::qcom_msm_watchdog_multiuple_regs_ =
    std::nullopt;

TEST_F(DevicetreeGenericWatchdogItemTest, NoWatchdog) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty();
  boot_shim::DevicetreeBootShim<boot_shim::GenericWatchdogItem<MmioHelper>> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG) {
      present = true;
    }
  }
  ASSERT_FALSE(present);
}

TEST_F(DevicetreeGenericWatchdogItemTest, QcomMsmWatchdog) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qcom_msm_watchdog();
  boot_shim::DevicetreeBootShim<boot_shim::GenericWatchdogItem<MmioHelper>> shim("test", fdt);
  std::vector<boot_shim::MmioRange> ranges;

  shim.set_mmio_observer([&ranges](boot_shim::MmioRange range) { ranges.push_back(range); });

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG) {
      present = true;
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_generic32_watchdog_t));
      auto* watchdog_item = reinterpret_cast<zbi_dcfg_generic32_watchdog_t*>(payload.data());

      EXPECT_EQ(watchdog_item->pet_action.addr, 0xf017004);
      EXPECT_EQ(watchdog_item->pet_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->pet_action.set_mask, 0x1);

      EXPECT_EQ(watchdog_item->enable_action.addr, 0xf017008);
      EXPECT_EQ(watchdog_item->enable_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->enable_action.set_mask, 0x1);

      EXPECT_EQ(watchdog_item->disable_action.addr, 0xf017008);
      EXPECT_EQ(watchdog_item->disable_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->disable_action.set_mask, 0x0);

      EXPECT_EQ(watchdog_item->watchdog_period_nsec, zx::msec(0x2490).to_nsecs());
      EXPECT_EQ(watchdog_item->flags, 0);
      EXPECT_EQ(watchdog_item->reserved, 0);
    }
  }
  ASSERT_TRUE(present);

  ASSERT_EQ(ranges.size(), 1);
  EXPECT_EQ(ranges[0].address, 0xf017000);
  EXPECT_EQ(ranges[0].size, 0x1000);
}

TEST_F(DevicetreeGenericWatchdogItemTest, QcomMsmWatchdogEnabled) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qcom_msm_watchdog();
  boot_shim::DevicetreeBootShim<boot_shim::GenericWatchdogItem<MmioHelper>> shim("test", fdt);
  std::vector<boot_shim::MmioRange> ranges;

  MmioHelper::value = 1234567891;
  shim.set_mmio_observer([&ranges](boot_shim::MmioRange range) { ranges.push_back(range); });

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG) {
      present = true;
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_generic32_watchdog_t));
      auto* watchdog_item = reinterpret_cast<zbi_dcfg_generic32_watchdog_t*>(payload.data());

      EXPECT_EQ(watchdog_item->pet_action.addr, 0xf017004);
      EXPECT_EQ(watchdog_item->pet_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->pet_action.set_mask, 0x1);

      EXPECT_EQ(watchdog_item->enable_action.addr, 0xf017008);
      EXPECT_EQ(watchdog_item->enable_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->enable_action.set_mask, 0x1);

      EXPECT_EQ(watchdog_item->disable_action.addr, 0xf017008);
      EXPECT_EQ(watchdog_item->disable_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->disable_action.set_mask, 0x0);

      EXPECT_EQ(watchdog_item->watchdog_period_nsec, zx::msec(0x2490).to_nsecs());
      EXPECT_EQ(watchdog_item->flags, ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG_FLAGS_ENABLED);
      EXPECT_EQ(watchdog_item->reserved, 0);
    }
  }
  ASSERT_TRUE(present);

  ASSERT_EQ(ranges.size(), 1);
  EXPECT_EQ(ranges[0].address, 0xf017000);
  EXPECT_EQ(ranges[0].size, 0x1000);
}

TEST_F(DevicetreeGenericWatchdogItemTest, QcomMsmWatchdogMultipleRegs) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qcom_msm_watchdog_multiuple_regs();
  boot_shim::DevicetreeBootShim<boot_shim::GenericWatchdogItem<MmioHelper>> shim("test", fdt);
  std::vector<boot_shim::MmioRange> ranges;

  shim.set_mmio_observer([&ranges](boot_shim::MmioRange range) { ranges.push_back(range); });

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG) {
      present = true;
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_generic32_watchdog_t));
      auto* watchdog_item = reinterpret_cast<zbi_dcfg_generic32_watchdog_t*>(payload.data());

      EXPECT_EQ(watchdog_item->pet_action.addr, 0xf018004);
      EXPECT_EQ(watchdog_item->pet_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->pet_action.set_mask, 0x1);

      EXPECT_EQ(watchdog_item->enable_action.addr, 0xf018008);
      EXPECT_EQ(watchdog_item->enable_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->enable_action.set_mask, 0x1);

      EXPECT_EQ(watchdog_item->disable_action.addr, 0xf018008);
      EXPECT_EQ(watchdog_item->disable_action.clr_mask, 0xffffffff);
      EXPECT_EQ(watchdog_item->disable_action.set_mask, 0x0);

      EXPECT_EQ(watchdog_item->watchdog_period_nsec, zx::msec(0x2490).to_nsecs());
      EXPECT_EQ(watchdog_item->flags, 0);
      EXPECT_EQ(watchdog_item->reserved, 0);
    }
  }
  ASSERT_TRUE(present);

  ASSERT_EQ(ranges.size(), 1);
  EXPECT_EQ(ranges[0].address, 0xf018000);
  EXPECT_EQ(ranges[0].size, 0x1000);
}

}  // namespace
