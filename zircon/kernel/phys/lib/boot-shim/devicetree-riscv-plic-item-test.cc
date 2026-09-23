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

class RiscvDevicetreePlicItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::RiscvDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("plic_riscv.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    plic_dtb_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    plic_dtb_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree plic() { return plic_dtb_->fdt(); }

 private:
  static std::optional<LoadedDtb> plic_dtb_;
};

std::optional<LoadedDtb> RiscvDevicetreePlicItemTest::plic_dtb_ = std::nullopt;

TEST_F(RiscvDevicetreePlicItemTest, BasicPlic) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = plic();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreePlicItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_err = fit::defer([&]() { image.ignore_error(); });

  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_RISCV_PLIC) {
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_riscv_plic_driver_t));
      auto* plic_dcfg = reinterpret_cast<zbi_dcfg_riscv_plic_driver_t*>(payload.data());
      EXPECT_EQ(plic_dcfg->mmio_phys, 0xc000000);
      EXPECT_EQ(plic_dcfg->size_bytes, 0x600000);
      EXPECT_EQ(plic_dcfg->num_irqs, 0x60);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreePlicItemTest, Qemu) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qemu_riscv();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreePlicItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_err = fit::defer([&]() { image.ignore_error(); });

  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_RISCV_PLIC) {
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_riscv_plic_driver_t));
      auto* plic_dcfg = reinterpret_cast<zbi_dcfg_riscv_plic_driver_t*>(payload.data());
      EXPECT_EQ(plic_dcfg->mmio_phys, 0xc000000);
      EXPECT_EQ(plic_dcfg->size_bytes, 0x600000);
      EXPECT_EQ(plic_dcfg->num_irqs, 0x60);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreePlicItemTest, VisionFive2) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = vision_five_2();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreePlicItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_err = fit::defer([&]() { image.ignore_error(); });

  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_RISCV_PLIC) {
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_riscv_plic_driver_t));
      auto* plic_dcfg = reinterpret_cast<zbi_dcfg_riscv_plic_driver_t*>(payload.data());
      EXPECT_EQ(plic_dcfg->mmio_phys, 0xc000000);
      EXPECT_EQ(plic_dcfg->size_bytes, 0x4000000);
      EXPECT_EQ(plic_dcfg->num_irqs, 0x88);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreePlicItemTest, HifiveSifiveUnmatched) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = sifive_hifive_unmatched();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreePlicItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_err = fit::defer([&]() { image.ignore_error(); });

  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_RISCV_PLIC) {
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_riscv_plic_driver_t));
      auto* plic_dcfg = reinterpret_cast<zbi_dcfg_riscv_plic_driver_t*>(payload.data());
      EXPECT_EQ(plic_dcfg->mmio_phys, 0xc000000);
      EXPECT_EQ(plic_dcfg->size_bytes, 0x4000000);
      EXPECT_EQ(plic_dcfg->num_irqs, 0x45);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreePlicItemTest, BananaPiF3) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = banana_pi_f3();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreePlicItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_err = fit::defer([&]() { image.ignore_error(); });

  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_RISCV_PLIC) {
      ASSERT_GE(payload.size(), sizeof(zbi_dcfg_riscv_plic_driver_t));
      auto* plic_dcfg = reinterpret_cast<zbi_dcfg_riscv_plic_driver_t*>(payload.data());
      EXPECT_EQ(plic_dcfg->mmio_phys, 0xe0000000);
      EXPECT_EQ(plic_dcfg->size_bytes, 0x4000000);
      EXPECT_EQ(plic_dcfg->num_irqs, 0x9f);
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(RiscvDevicetreePlicItemTest, MissingNode) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreePlicItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  auto clear_err = fit::defer([&]() { image.ignore_error(); });

  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_RISCV_PLIC);
  }
}

}  // namespace
