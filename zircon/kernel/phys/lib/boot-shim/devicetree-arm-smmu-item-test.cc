// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/fit/defer.h>
#include <lib/fit/result.h>

namespace {

using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

class TestAllocator {
 public:
  TestAllocator() = default;
  TestAllocator(TestAllocator&& other) {
    allocs_ = std::move(other.allocs_);
    other.allocs_.clear();
  }

  ~TestAllocator() {
    for (auto* alloc : allocs_) {
      free(alloc);
    }
  }

  void* operator()(size_t size, size_t alignment, fbl::AllocChecker& ac) {
    void* alloc = malloc(size + alignment);
    allocs_.push_back(alloc);
    ac.arm(size + alignment, alloc != nullptr);
    return reinterpret_cast<void*>((reinterpret_cast<uintptr_t>(alloc) + alignment) &
                                   ~(alignment - 1));
  }

 private:
  std::vector<void*> allocs_;
};

class ArmDevicetreeSmmuItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::ArmDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  using DTResult = fit::result<std::string, LoadedDtb>;
  static void SetUpTestSuite() { Mixin::SetUpTestSuite(); }
  static void TearDownTestSuite() { Mixin::TearDownTestSuite(); }

  static void CheckSmmuItem(const zbi_dcfg_arm_smmu_driver_t& item,
                            const zbi_dcfg_arm_smmu_driver_t& expected) {
    EXPECT_EQ(expected.mmio_phys, item.mmio_phys);
    EXPECT_EQ(expected.mmio_phys, item.mmio_phys);
    EXPECT_EQ(expected.num_context_banks_override, item.num_context_banks_override);
    EXPECT_EQ(expected.num_smr_override, item.num_smr_override);
    EXPECT_EQ(expected.irq_cnt, item.irq_cnt);
    EXPECT_EQ(expected.global_irq_cnt, item.global_irq_cnt);
    EXPECT_BYTES_EQ(expected.irqs, item.irqs, sizeof(item.irqs));
    EXPECT_EQ(expected.handoff_smr_cnt, item.handoff_smr_cnt);
    EXPECT_BYTES_EQ(expected.handoff_smrs, item.handoff_smrs, sizeof(item.handoff_smrs));
  }

  DTResult LoadDevicetree(const char* name) {
    auto loaded_dtb = LoadDtb(name);
    if (!loaded_dtb.is_ok()) {
      return loaded_dtb.take_error();
    }
    return fit::success(std::move(loaded_dtb).value());
  }

  void DoBasicTest(const char* dt_name, std::vector<const zbi_dcfg_arm_smmu_driver_t*> expected) {
    DTResult maybe_devtree = LoadDevicetree(dt_name);
    ASSERT_TRUE(maybe_devtree.is_ok(), "%s", maybe_devtree.error_value().c_str());

    std::array<std::byte, 4096> image_buffer;
    std::vector<void*> allocs;
    zbitl::Image<std::span<std::byte>> image(image_buffer);
    ASSERT_TRUE(image.clear().is_ok());

    boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeSmmuItem> shim("test",
                                                                         maybe_devtree->fdt());
    shim.set_allocator(TestAllocator());

    ASSERT_TRUE(shim.Init());
    auto clear_errors = fit::defer([&]() { image.ignore_error(); });
    ASSERT_TRUE(shim.AppendItems(image).is_ok());

    size_t found{0};
    for (auto [header, payload] : image) {
      if (header->type == ZBI_TYPE_KERNEL_DRIVER && header->extra == ZBI_KERNEL_DRIVER_ARM_SMMU) {
        const zbi_dcfg_arm_smmu_driver_t* items =
            reinterpret_cast<const zbi_dcfg_arm_smmu_driver_t*>(payload.data());
        const size_t avail = payload.size_bytes() / sizeof(zbi_dcfg_arm_smmu_driver_t);

        for (size_t i = 0; i < avail; ++i) {
          ASSERT_LT(found, expected.size());
          const zbi_dcfg_arm_smmu_driver_t& item = items[i];
          const zbi_dcfg_arm_smmu_driver_t& e = *expected[found++];
          CheckSmmuItem(item, e);
        }
      }
    }
    EXPECT_EQ(expected.size(), found);
  }
};

// Test a basic SMMU description, similar to what we would expect to see in the field.
TEST_F(ArmDevicetreeSmmuItemTest, Simple) {
  constexpr zbi_dcfg_arm_smmu_driver_t kExpected{
      .mmio_phys = 0x4e54000,
      .num_context_banks_override = 4,
      .num_smr_override = 8,
      .irq_cnt = 5,
      .global_irq_cnt = 1,
      .irqs =
          {
              {.num = 0x71, .flags = 0x04},
              {.num = 0x77, .flags = 0x04},
              {.num = 0x78, .flags = 0x04},
              {.num = 0x79, .flags = 0x04},
              {.num = 0x7a, .flags = 0x04},
          },
      .handoff_smr_cnt = 1,
      .handoff_smrs{0x00020420},
  };

  DoBasicTest("smmu_simple.dtb", {&kExpected});
}

// Test a SMMU description which contains nothing but the MMIO register address.
// We should see defaults (zero) for everything else we parse.
TEST_F(ArmDevicetreeSmmuItemTest, Defaults) {
  constexpr zbi_dcfg_arm_smmu_driver_t kExpected{
      .mmio_phys = 0x4e54000,
      .num_context_banks_override = 0,
      .num_smr_override = 0,
      .irq_cnt = 0,
      .global_irq_cnt = 0,
      .irqs = {},
      .handoff_smr_cnt = 0,
      .handoff_smrs{},
  };

  DoBasicTest("smmu_defaults.dtb", {&kExpected});
}

// Test a SMMU description where the interrupt and handoff SMR vectors are asked to hold more
// entries than should be possible given the limits of the fixed structure size.  We expect the
// vectors to be truncated to hold the max that they can.
TEST_F(ArmDevicetreeSmmuItemTest, Overflow) {
  constexpr zbi_dcfg_arm_smmu_driver_t kExpected{
      .mmio_phys = 0x4e54000,
      .num_context_banks_override = 0,
      .num_smr_override = 0,
      .irq_cnt = ZBI_KERNEL_DRIVER_SMMU_MAX_IRQS,
      .global_irq_cnt = 1,
      .irqs =
          {
              {.num = 0x21, .flags = 0x04}, {.num = 0x22, .flags = 0x04},
              {.num = 0x23, .flags = 0x04}, {.num = 0x24, .flags = 0x04},
              {.num = 0x25, .flags = 0x04}, {.num = 0x26, .flags = 0x04},
              {.num = 0x27, .flags = 0x04}, {.num = 0x28, .flags = 0x04},
              {.num = 0x29, .flags = 0x04}, {.num = 0x2a, .flags = 0x04},
              {.num = 0x2b, .flags = 0x04}, {.num = 0x2c, .flags = 0x04},
              {.num = 0x2d, .flags = 0x04}, {.num = 0x2e, .flags = 0x04},
              {.num = 0x2f, .flags = 0x04}, {.num = 0x30, .flags = 0x04},
              {.num = 0x31, .flags = 0x04}, {.num = 0x32, .flags = 0x04},
              {.num = 0x33, .flags = 0x04}, {.num = 0x34, .flags = 0x04},
              {.num = 0x35, .flags = 0x04}, {.num = 0x36, .flags = 0x04},
              {.num = 0x37, .flags = 0x04}, {.num = 0x38, .flags = 0x04},
              {.num = 0x39, .flags = 0x04}, {.num = 0x3a, .flags = 0x04},
              {.num = 0x3b, .flags = 0x04}, {.num = 0x3c, .flags = 0x04},
              {.num = 0x3d, .flags = 0x04}, {.num = 0x3e, .flags = 0x04},
              {.num = 0x3f, .flags = 0x04}, {.num = 0x40, .flags = 0x04},
              {.num = 0x41, .flags = 0x04}, {.num = 0x42, .flags = 0x04},
              {.num = 0x43, .flags = 0x04}, {.num = 0x44, .flags = 0x04},
              {.num = 0x45, .flags = 0x04}, {.num = 0x46, .flags = 0x04},
              {.num = 0x47, .flags = 0x04}, {.num = 0x48, .flags = 0x04},
              {.num = 0x49, .flags = 0x04}, {.num = 0x4a, .flags = 0x04},
              {.num = 0x4b, .flags = 0x04}, {.num = 0x4c, .flags = 0x04},
              {.num = 0x4d, .flags = 0x04}, {.num = 0x4e, .flags = 0x04},
              {.num = 0x4f, .flags = 0x04}, {.num = 0x50, .flags = 0x04},
              {.num = 0x51, .flags = 0x04}, {.num = 0x52, .flags = 0x04},
              {.num = 0x53, .flags = 0x04}, {.num = 0x54, .flags = 0x04},
              {.num = 0x55, .flags = 0x04}, {.num = 0x56, .flags = 0x04},
              {.num = 0x57, .flags = 0x04}, {.num = 0x58, .flags = 0x04},
              {.num = 0x59, .flags = 0x04}, {.num = 0x5a, .flags = 0x04},
              {.num = 0x5b, .flags = 0x04}, {.num = 0x5c, .flags = 0x04},
              {.num = 0x5d, .flags = 0x04}, {.num = 0x5e, .flags = 0x04},
              {.num = 0x5f, .flags = 0x04}, {.num = 0x60, .flags = 0x04},
              {.num = 0x61, .flags = 0x04}, {.num = 0x62, .flags = 0x04},
              {.num = 0x63, .flags = 0x04}, {.num = 0x64, .flags = 0x04},
              {.num = 0x65, .flags = 0x04}, {.num = 0x66, .flags = 0x04},
              {.num = 0x67, .flags = 0x04}, {.num = 0x68, .flags = 0x04},
              {.num = 0x69, .flags = 0x04}, {.num = 0x6a, .flags = 0x04},
              {.num = 0x6b, .flags = 0x04}, {.num = 0x6c, .flags = 0x04},
              {.num = 0x6d, .flags = 0x04}, {.num = 0x6e, .flags = 0x04},
              {.num = 0x6f, .flags = 0x04}, {.num = 0x70, .flags = 0x04},
              {.num = 0x71, .flags = 0x04}, {.num = 0x72, .flags = 0x04},
              {.num = 0x73, .flags = 0x04}, {.num = 0x74, .flags = 0x04},
              {.num = 0x75, .flags = 0x04}, {.num = 0x76, .flags = 0x04},
              {.num = 0x77, .flags = 0x04}, {.num = 0x78, .flags = 0x04},
              {.num = 0x79, .flags = 0x04}, {.num = 0x7a, .flags = 0x04},
              {.num = 0x7b, .flags = 0x04}, {.num = 0x7c, .flags = 0x04},
              {.num = 0x7d, .flags = 0x04}, {.num = 0x7e, .flags = 0x04},
              {.num = 0x7f, .flags = 0x04}, {.num = 0x80, .flags = 0x04},
              {.num = 0x81, .flags = 0x04}, {.num = 0x82, .flags = 0x04},
              {.num = 0x83, .flags = 0x04}, {.num = 0x84, .flags = 0x04},
              {.num = 0x85, .flags = 0x04}, {.num = 0x86, .flags = 0x04},
              {.num = 0x87, .flags = 0x04}, {.num = 0x88, .flags = 0x04},
              {.num = 0x89, .flags = 0x04}, {.num = 0x8a, .flags = 0x04},
              {.num = 0x8b, .flags = 0x04}, {.num = 0x8c, .flags = 0x04},
              {.num = 0x8d, .flags = 0x04}, {.num = 0x8e, .flags = 0x04},
              {.num = 0x8f, .flags = 0x04}, {.num = 0x90, .flags = 0x04},
              {.num = 0x91, .flags = 0x04}, {.num = 0x92, .flags = 0x04},
              {.num = 0x93, .flags = 0x04}, {.num = 0x94, .flags = 0x04},
              {.num = 0x95, .flags = 0x04}, {.num = 0x96, .flags = 0x04},
              {.num = 0x97, .flags = 0x04}, {.num = 0x98, .flags = 0x04},
              {.num = 0x99, .flags = 0x04}, {.num = 0x9a, .flags = 0x04},
              {.num = 0x9b, .flags = 0x04}, {.num = 0x9c, .flags = 0x04},
              {.num = 0x9d, .flags = 0x04}, {.num = 0x9e, .flags = 0x04},
              {.num = 0x9f, .flags = 0x04}, {.num = 0xa0, .flags = 0x04},
          },
      .handoff_smr_cnt = ZBI_KERNEL_DRIVER_SMMU_MAX_HANDOFF_SMRS,
      .handoff_smrs{
          0x00000420,
          0x00000421,
          0x00000422,
          0x00000423,
          0x00000424,
          0x00000425,
          0x00000426,
          0x00000427,
          0x00000428,
          0x00000429,
          0x0000042a,
          0x0000042b,
          0x0000042c,
          0x0000042d,
          0x0000042e,
          0x0000042f,
      },
  };

  DoBasicTest("smmu_overflow.dtb", {&kExpected});
}

// Test a SMMU description that includes a bad IRQ index.  We expect the
// reported IRQ index to be set to zero (invalid), but not removed from the
// array as the positions of the interrupt numbers in the array is important (it
// defines which context banks they apply to in SMMUv2.
TEST_F(ArmDevicetreeSmmuItemTest, BadIrqIndex) {
  constexpr zbi_dcfg_arm_smmu_driver_t kExpected{
      .mmio_phys = 0x4e54000,
      .num_context_banks_override = 0,
      .num_smr_override = 0,
      .irq_cnt = 5,
      .global_irq_cnt = 1,
      .irqs =
          {
              {.num = 0x21, .flags = 0x04},
              {.num = 0x22, .flags = 0x04},
              {.num = 0x00, .flags = 0x00},
              {.num = 0x24, .flags = 0x04},
              {.num = 0x25, .flags = 0x04},
          },
      .handoff_smr_cnt = 0,
      .handoff_smrs{},
  };

  DoBasicTest("smmu_bad_irq.dtb", {&kExpected});
}

// Test a situation where we expect to have multiple SMMUs in the same devicetree.
TEST_F(ArmDevicetreeSmmuItemTest, Multi) {
  constexpr zbi_dcfg_arm_smmu_driver_t kFirst{
      .mmio_phys = 0x4e54000,
      .num_context_banks_override = 4,
      .num_smr_override = 8,
      .irq_cnt = 5,
      .global_irq_cnt = 1,
      .irqs =
          {
              {.num = 0x71, .flags = 0x04},
              {.num = 0x77, .flags = 0x04},
              {.num = 0x78, .flags = 0x04},
              {.num = 0x79, .flags = 0x04},
              {.num = 0x7a, .flags = 0x04},
          },
      .handoff_smr_cnt = 1,
      .handoff_smrs{0x00020420},
  };

  constexpr zbi_dcfg_arm_smmu_driver_t kSecond{
      .mmio_phys = 0xabcd000,
      .num_context_banks_override = 7,
      .num_smr_override = 0x17,
      .irq_cnt = 9,
      .global_irq_cnt = 2,
      .irqs =
          {
              {.num = 0xA1, .flags = 0x04},
              {.num = 0xA2, .flags = 0x04},
              {.num = 0xB0, .flags = 0x04},
              {.num = 0xB1, .flags = 0x04},
              {.num = 0xB2, .flags = 0x04},
              {.num = 0xB3, .flags = 0x04},
              {.num = 0xB4, .flags = 0x04},
              {.num = 0xB5, .flags = 0x04},
              {.num = 0xB6, .flags = 0x04},
          },
      .handoff_smr_cnt = 0,
      .handoff_smrs{},
  };

  DoBasicTest("smmu_multi.dtb", {&kFirst, &kSecond});
}

}  // namespace
