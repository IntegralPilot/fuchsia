// Copyright 2025 The Fuchsia Authors
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

class ArmDevicetreeQcomRngItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("arm_qcom_rng_configured.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    enabled_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_qcom_rng.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    not_enabled_ = std::move(loaded_dtb).value();
  }
  static void TearDownTestSuite() { Mixin::TearDownTestSuite(); }

  devicetree::Devicetree qcom_rng_enabled() { return enabled_->fdt(); }
  devicetree::Devicetree qcom_rng_not_enabled() { return not_enabled_->fdt(); }

  auto get_mmio_observer() {
    return [this](const boot_shim::MmioRange& r) { ranges_.push_back(r); };
  }

  const auto& ranges() { return ranges_; }

 private:
  static inline std::optional<LoadedDtb> enabled_;
  static inline std::optional<LoadedDtb> not_enabled_;

  std::vector<boot_shim::MmioRange> ranges_;
};

TEST_F(ArmDevicetreeQcomRngItemTest, MissingNode) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeQcomRngItem> shim("test", fdt);
  shim.set_mmio_observer(get_mmio_observer());
  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_QCOM_RNG);
  }
  ASSERT_EQ(ranges().size(), 0);
}

TEST_F(ArmDevicetreeQcomRngItemTest, NodeWithoutUnconfiguredProperty) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qcom_rng_enabled();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeQcomRngItem> shim("test", fdt);
  shim.set_mmio_observer(get_mmio_observer());

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_QCOM_RNG) {
      ASSERT_EQ(payload.size(), sizeof(zbi_dcfg_qcom_rng_t));
      zbi_dcfg_qcom_rng_t* dcfg = reinterpret_cast<zbi_dcfg_qcom_rng_t*>(payload.data());
      EXPECT_EQ(dcfg->mmio_phys, 0x1234);
      EXPECT_EQ(dcfg->flags, ZBI_QCOM_RNG_FLAGS_ENABLED);
      present = true;
    }
  }
  ASSERT_TRUE(present);
  ASSERT_EQ(ranges().size(), 1);
  EXPECT_EQ(ranges()[0].address, 0x1234);
  EXPECT_EQ(ranges()[0].size, 0x1000);
}

TEST_F(ArmDevicetreeQcomRngItemTest, NodeWithUnconfiguredProperty) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qcom_rng_not_enabled();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeQcomRngItem> shim("test", fdt);
  shim.set_mmio_observer(get_mmio_observer());

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  bool present = false;
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_QCOM_RNG) {
      ASSERT_EQ(payload.size(), sizeof(zbi_dcfg_qcom_rng_t));
      zbi_dcfg_qcom_rng_t* dcfg = reinterpret_cast<zbi_dcfg_qcom_rng_t*>(payload.data());
      EXPECT_EQ(dcfg->mmio_phys, 0x1234);
      EXPECT_EQ(dcfg->flags, 0);
      present = true;
    }
  }
  ASSERT_TRUE(present);
  ASSERT_EQ(ranges().size(), 1);
  EXPECT_EQ(ranges()[0].address, 0x1234);
  EXPECT_EQ(ranges()[0].size, 0x1000);
}

}  // namespace
