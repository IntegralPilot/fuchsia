// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/fit/defer.h>
#include <lib/zbitl/image.h>

namespace {

using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

class ArmDevicetreeTimerItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::ArmDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("arm_timer.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    timer_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_timer_no_frequency_override.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    timer_no_frequency_override_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    timer_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree timer() { return timer_->fdt(); }
  devicetree::Devicetree timer_no_frequency_override() {
    return timer_no_frequency_override_->fdt();
  }

 private:
  static std::optional<LoadedDtb> timer_;
  static std::optional<LoadedDtb> timer_no_frequency_override_;
};

std::optional<LoadedDtb> ArmDevicetreeTimerItemTest::timer_ = std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTimerItemTest::timer_no_frequency_override_ = std::nullopt;

TEST_F(ArmDevicetreeTimerItemTest, TimerWithFrequencyOverride) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = timer();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER) {
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_driver_t));
      auto* timer_item = reinterpret_cast<zbi_dcfg_arm_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(timer_item->freq_override, 1234);
      EXPECT_EQ(timer_item->irq_phys, 27);
      EXPECT_EQ(timer_item->irq_sphys, 26);
      EXPECT_EQ(timer_item->irq_virt, 28);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerItemTest, TimerWithoutFrequencyOverride) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = timer_no_frequency_override();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER) {
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_driver_t));
      auto* timer_item = reinterpret_cast<zbi_dcfg_arm_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(timer_item->freq_override, 0);
      EXPECT_EQ(timer_item->irq_phys, 27);
      EXPECT_EQ(timer_item->irq_sphys, 26);
      EXPECT_EQ(timer_item->irq_virt, 28);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerItemTest, Qemu) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qemu_arm_gic3();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER) {
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_driver_t));
      auto* timer_item = reinterpret_cast<zbi_dcfg_arm_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(timer_item->freq_override, 0);
      EXPECT_EQ(timer_item->irq_phys, 30);
      EXPECT_EQ(timer_item->irq_sphys, 29);
      EXPECT_EQ(timer_item->irq_virt, 27);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerItemTest, Crosvm) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = crosvm_arm();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER) {
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_driver_t));
      auto* timer_item = reinterpret_cast<zbi_dcfg_arm_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(timer_item->freq_override, 0);
      EXPECT_EQ(timer_item->irq_phys, 30);
      EXPECT_EQ(timer_item->irq_sphys, 29);
      EXPECT_EQ(timer_item->irq_virt, 27);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerItemTest, KhadasVim3) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = khadas_vim3();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", fdt);

  shim.Init();

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER) {
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_driver_t));
      auto* timer_item = reinterpret_cast<zbi_dcfg_arm_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(timer_item->freq_override, 0);
      EXPECT_EQ(timer_item->irq_phys, 30);
      EXPECT_EQ(timer_item->irq_sphys, 29);
      EXPECT_EQ(timer_item->irq_virt, 27);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerItemTest, MissingNode) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER);
  }
}

}  // namespace
