// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/devicetree/testing/loaded-dtb.h>
#include <lib/zbi-format/cpu.h>

#include <optional>
#include <span>

#include <zxtest/zxtest.h>

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_TESTING_DEVICETREE_TEST_FIXTURE_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_TESTING_DEVICETREE_TEST_FIXTURE_H_

namespace boot_shim::testing {

using devicetree::testing::LoadDtb;
using devicetree::testing::LoadedDtb;

// Common set of synthetic DTBs.
class SyntheticDevicetreeTest {
 public:
  static void SetUpTestSuite() {
    auto loaded_dtb = LoadDtb("empty.dtb");

    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    empty_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_gic2_no_msi.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    arm_gic2_no_msi_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_gic3_stride.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    arm_gic3_stride_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_gic3_four_stride.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    arm_gic3_four_stride_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_gic3_subsumed_stride.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    arm_gic3_subsumed_stride_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    empty_dtb_ = std::nullopt;
    arm_gic2_no_msi_ = std::nullopt;
    arm_gic3_four_stride_ = std::nullopt;
    arm_gic3_stride_ = std::nullopt;
    arm_gic3_subsumed_stride_ = std::nullopt;
  }

  auto empty_fdt() { return empty_dtb_->fdt(); }
  auto arm_gic2_no_msi() { return arm_gic2_no_msi_->fdt(); }
  auto arm_gic3_stride() { return arm_gic3_stride_->fdt(); }
  auto arm_gic3_four_stride() { return arm_gic3_four_stride_->fdt(); }
  auto arm_gic3_subsumed_stride() { return arm_gic3_subsumed_stride_->fdt(); }

 private:
  static std::optional<LoadedDtb> empty_dtb_;
  static std::optional<LoadedDtb> arm_gic2_no_msi_;
  static std::optional<LoadedDtb> arm_gic3_stride_;
  static std::optional<LoadedDtb> arm_gic3_four_stride_;
  static std::optional<LoadedDtb> arm_gic3_subsumed_stride_;
};

// Devicetree Test fixture that provides members to existing ARM dtbs.
class ArmDevicetreeTest {
 public:
  static void SetUpTestSuite() {
    auto loaded_dtb = LoadDtb("crosvm-arm.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    crosvm_arm_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("qemu-arm-gic3.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    qemu_arm_gic3_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("qemu-arm-gic2.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    qemu_arm_gic2_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("khadas-vim3.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    khadas_vim3_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    qemu_arm_gic3_ = std::nullopt;
    qemu_arm_gic2_ = std::nullopt;
    crosvm_arm_ = std::nullopt;
    khadas_vim3_ = std::nullopt;
  }

  devicetree::Devicetree qemu_arm_gic3() { return qemu_arm_gic3_->fdt(); }
  devicetree::Devicetree qemu_arm_gic2() { return qemu_arm_gic2_->fdt(); }
  devicetree::Devicetree crosvm_arm() { return crosvm_arm_->fdt(); }
  devicetree::Devicetree khadas_vim3() { return khadas_vim3_->fdt(); }

 private:
  static std::optional<LoadedDtb> crosvm_arm_;
  static std::optional<LoadedDtb> qemu_arm_gic3_;
  static std::optional<LoadedDtb> qemu_arm_gic2_;
  static std::optional<LoadedDtb> khadas_vim3_;
};

// Devicetree Test fixture that provides members to existing RISCV dtbs.
class RiscvDevicetreeTest {
 public:
  static void SetUpTestSuite() {
    auto loaded_dtb = LoadDtb("qemu-riscv.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    qemu_riscv_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("sifive-hifive-unmatched.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    sifive_hifive_unmatched_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("vision-five-2.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    vision_five_2_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("banana-pi-f3.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    banana_pi_f3_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    qemu_riscv_ = std::nullopt;
    vision_five_2_ = std::nullopt;
    sifive_hifive_unmatched_ = std::nullopt;
    banana_pi_f3_ = std::nullopt;
  }

  devicetree::Devicetree qemu_riscv() { return qemu_riscv_->fdt(); }
  devicetree::Devicetree sifive_hifive_unmatched() { return sifive_hifive_unmatched_->fdt(); }
  devicetree::Devicetree vision_five_2() { return vision_five_2_->fdt(); }
  devicetree::Devicetree banana_pi_f3() { return banana_pi_f3_->fdt(); }

 private:
  static std::optional<LoadedDtb> qemu_riscv_;
  static std::optional<LoadedDtb> sifive_hifive_unmatched_;
  static std::optional<LoadedDtb> vision_five_2_;
  static std::optional<LoadedDtb> banana_pi_f3_;
};

// Template that combines architecture fixtures into a single Test fixture.
template <typename... Base>
class TestMixin : public zxtest::Test, public Base... {
 public:
  using Mixin = TestMixin;
  static_assert(sizeof...(Base) > 0);

  static void SetUpTestSuite() { (SetUpOne<Base>() && ...); }
  static void TearDownTestSuite() { (TearDownOne<Base>() && ...); }

 private:
  template <typename T>
  static bool SetUpOne() {
    T::SetUpTestSuite();
    return true;
  }

  template <typename T>
  static bool TearDownOne() {
    T::SetUpTestSuite();
    return true;
  }
};

void CheckCpuTopology(std::span<const zbi_topology_node_t> actual_nodes,
                      std::span<const zbi_topology_node_t> expected_nodes);

void CheckMmioRanges(std::span<const boot_shim::MmioRange> actual,
                     std::span<const boot_shim::MmioRange> expected);

}  // namespace boot_shim::testing

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_TESTING_DEVICETREE_TEST_FIXTURE_H_
