// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/fit/defer.h>

namespace {

using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

class RiscvDevicetreeTimerItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::RiscvDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("cpus_riscv.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    cpus_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    cpus_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree cpus() { return cpus_->fdt(); }

 private:
  static std::optional<LoadedDtb> cpus_;
};

std::optional<LoadedDtb> RiscvDevicetreeTimerItemTest::cpus_ = std::nullopt;

TEST_F(RiscvDevicetreeTimerItemTest, MissingNode) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER);
  }
}

TEST_F(RiscvDevicetreeTimerItemTest, TimerFromCpus) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = cpus();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER) {
      EXPECT_GE(payload.size(), sizeof(zbi_dcfg_riscv_generic_timer_driver_t));
      auto* dcfg = reinterpret_cast<zbi_dcfg_riscv_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(dcfg->freq_hz, 0x989680);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreeTimerItemTest, Qemu) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qemu_riscv();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER) {
      EXPECT_GE(payload.size(), sizeof(zbi_dcfg_riscv_generic_timer_driver_t));
      auto* dcfg = reinterpret_cast<zbi_dcfg_riscv_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(dcfg->freq_hz, 0x989680);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreeTimerItemTest, VisionFive2) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = vision_five_2();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER) {
      EXPECT_GE(payload.size(), sizeof(zbi_dcfg_riscv_generic_timer_driver_t));
      auto* dcfg = reinterpret_cast<zbi_dcfg_riscv_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(dcfg->freq_hz, 0x3D0900);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreeTimerItemTest, SifiveHifiveUnmatched) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = sifive_hifive_unmatched();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER) {
      EXPECT_GE(payload.size(), sizeof(zbi_dcfg_riscv_generic_timer_driver_t));
      auto* dcfg = reinterpret_cast<zbi_dcfg_riscv_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(dcfg->freq_hz, 0xF4240);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreeTimerItemTest, BananaPiF3) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = banana_pi_f3();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER) {
      EXPECT_GE(payload.size(), sizeof(zbi_dcfg_riscv_generic_timer_driver_t));
      auto* dcfg = reinterpret_cast<zbi_dcfg_riscv_generic_timer_driver_t*>(payload.data());
      EXPECT_EQ(dcfg->freq_hz, 0x16E3600);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

}  // namespace
