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

#include <span>

namespace {

using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

class ArmDevicetreeTimerMmioItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::ArmDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("arm_timer_mmio.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    timer_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_timer_mmio_no_frequency_override.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    timer_no_frequency_override_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_timer_mmio_no_frames.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    timer_no_frames_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_timer_mmio_invalid_frames.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    timer_invalid_frames_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    timer_ = std::nullopt;
    timer_invalid_frames_ = std::nullopt;
    timer_no_frames_ = std::nullopt;
    timer_no_frequency_override_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree timer() { return timer_->fdt(); }
  devicetree::Devicetree timer_no_frequency_override() {
    return timer_no_frequency_override_->fdt();
  }
  devicetree::Devicetree timer_invalid_frames() { return timer_invalid_frames_->fdt(); }
  devicetree::Devicetree timer_no_frames() { return timer_no_frames_->fdt(); }

 private:
  static std::optional<LoadedDtb> timer_;
  static std::optional<LoadedDtb> timer_no_frequency_override_;
  static std::optional<LoadedDtb> timer_invalid_frames_;
  static std::optional<LoadedDtb> timer_no_frames_;
};

std::optional<LoadedDtb> ArmDevicetreeTimerMmioItemTest::timer_ = std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTimerMmioItemTest::timer_no_frequency_override_ =
    std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTimerMmioItemTest::timer_no_frames_ = std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTimerMmioItemTest::timer_invalid_frames_ = std::nullopt;

TEST_F(ArmDevicetreeTimerMmioItemTest, MissingNode) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerMmioItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO);
  }
}

TEST_F(ArmDevicetreeTimerMmioItemTest, NoFrames) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = timer_no_frames();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerMmioItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO);
  }
}

struct IrqInfo {
  uint32_t irq;
  uint32_t irq_flags;
};

struct ExpectedFrame {
  uint8_t id;
  uint32_t mmio_phys_el1;
  std::optional<uint32_t> mmio_phys_el0;
  IrqInfo phys_irq;
  std::optional<IrqInfo> virt_irq = std::nullopt;
};

void CheckGenericTimerMmioItem(const zbi_dcfg_arm_generic_timer_mmio_driver_t& item,
                               uint32_t mmio_phys, std::optional<uint32_t> clock_frequency,
                               std::span<ExpectedFrame> frames, std::span<ExpectedFrame> disabled) {
  ASSERT_EQ(item.mmio_phys, mmio_phys);
  ASSERT_EQ(item.frequency, clock_frequency.value_or(0));

  // Check that the number of active frames matches the number of expected frames.
  uint32_t num_active = std::popcount(item.active_frames_mask);
  ASSERT_EQ(num_active, frames.size());

  IrqInfo empty = {
      .irq = 0,
      .irq_flags = 0,
  };

  for (const auto& expected_frame : frames) {
    ASSERT_LT(expected_frame.id, 8);
    uint8_t id_mask = static_cast<uint8_t>(1 << expected_frame.id);
    ASSERT_NE(item.active_frames_mask & id_mask, 0);
    const auto& timer_frame = item.frames[expected_frame.id];

    EXPECT_EQ(timer_frame.mmio_phys_el1, expected_frame.mmio_phys_el1);
    EXPECT_EQ(timer_frame.mmio_phys_el0, expected_frame.mmio_phys_el0.value_or(0));
    EXPECT_EQ(timer_frame.irq_phys, expected_frame.phys_irq.irq);
    EXPECT_EQ(timer_frame.irq_phys_flags, expected_frame.phys_irq.irq_flags);
    EXPECT_EQ(timer_frame.irq_virt, expected_frame.virt_irq.value_or(empty).irq);
    EXPECT_EQ(timer_frame.irq_virt_flags, expected_frame.virt_irq.value_or(empty).irq_flags);
  }

  // Check that all inactive frames are presented as disabled.
  for (uint8_t i = 0; i < 8; i++) {
    uint8_t mask = static_cast<uint8_t>(1 << i);
    if ((mask & item.active_frames_mask) != 0) {
      continue;
    }

    bool done = false;
    auto inactive_frame = item.frames[i];

    // Disabled frames keep their MMIO regions.
    for (const auto& frame : disabled) {
      if (frame.id != i) {
        continue;
      }
      done = true;
      EXPECT_EQ(inactive_frame.mmio_phys_el1, frame.mmio_phys_el1);
      EXPECT_EQ(inactive_frame.mmio_phys_el0, frame.mmio_phys_el0.value_or(0));
      EXPECT_EQ(inactive_frame.irq_phys, frame.phys_irq.irq);
      EXPECT_EQ(inactive_frame.irq_phys_flags, frame.phys_irq.irq_flags);
      EXPECT_EQ(inactive_frame.irq_virt, frame.virt_irq.value_or(empty).irq);
      EXPECT_EQ(inactive_frame.irq_virt_flags, frame.virt_irq.value_or(empty).irq_flags);
    }

    if (done) {
      continue;
    }
    EXPECT_EQ(inactive_frame.mmio_phys_el1, 0);
    EXPECT_EQ(inactive_frame.mmio_phys_el0, 0);
    EXPECT_EQ(inactive_frame.irq_phys, 0);
    EXPECT_EQ(inactive_frame.irq_phys_flags, 0);
    EXPECT_EQ(inactive_frame.irq_virt, 0);
    EXPECT_EQ(inactive_frame.irq_virt_flags, 0);
  }
}

TEST_F(ArmDevicetreeTimerMmioItemTest, MultipleFramesWithClockFrequencyOverride) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto kExpectedFrames = std::to_array<ExpectedFrame>({
      {
          .id = 1,
          .mmio_phys_el1 = 0x121000,
          .mmio_phys_el0 = 0x122000,
          .phys_irq =
              {
                  .irq = 40,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },

          .virt_irq =
              IrqInfo{
                  .irq = 39,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 2,
          .mmio_phys_el1 = 0x123000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 41,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 4,
          .mmio_phys_el1 = 0x124000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 42,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 6,
          .mmio_phys_el1 = 0x125000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 43,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 7,
          .mmio_phys_el1 = 0x126000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 44,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
  });

  auto kInactiveFrames = std::to_array<ExpectedFrame>({
      {
          .id = 3,
          .mmio_phys_el1 = 0x127000,
          .mmio_phys_el0 = 0x128000,
          .phys_irq =
              {
                  .irq = 45,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },

      {
          .id = 0,
          .mmio_phys_el1 = 0x129000,
          .mmio_phys_el0 = 0,
          .phys_irq =
              {
                  .irq = 46,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
  });

  auto fdt = timer();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerMmioItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO) {
      present = true;
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_mmio_driver_t));
      const auto* item =
          reinterpret_cast<const zbi_dcfg_arm_generic_timer_mmio_driver_t*>(payload.data());
      CheckGenericTimerMmioItem(*item, 0x120000, 0x124f800, kExpectedFrames, kInactiveFrames);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerMmioItemTest, MultipleFramesWithoutClockFrequencyOverride) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto kExpectedFrames = std::to_array<ExpectedFrame>({
      {
          .id = 1,
          .mmio_phys_el1 = 0x12341000,
          .mmio_phys_el0 = 0x12342000,
          .phys_irq =
              {
                  .irq = 40,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },

          .virt_irq =
              IrqInfo{
                  .irq = 39,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 2,
          .mmio_phys_el1 = 0x12343000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 41,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 4,
          .mmio_phys_el1 = 0x12344000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 42,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 6,
          .mmio_phys_el1 = 0x12345000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 43,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 7,
          .mmio_phys_el1 = 0x12346000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 44,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
  });

  auto kInactiveFrames = std::to_array<ExpectedFrame>({
      {
          .id = 3,
          .mmio_phys_el1 = 0x12347000,
          .mmio_phys_el0 = 0x12348000,
          .phys_irq =
              {
                  .irq = 45,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },

      {
          .id = 0,
          .mmio_phys_el1 = 0x12349000,
          .mmio_phys_el0 = 0,
          .phys_irq =
              {
                  .irq = 46,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
  });

  auto fdt = timer_no_frequency_override();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerMmioItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO) {
      present = true;
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_mmio_driver_t));
      const auto* item =
          reinterpret_cast<const zbi_dcfg_arm_generic_timer_mmio_driver_t*>(payload.data());
      CheckGenericTimerMmioItem(*item, 0x12340000, std::nullopt, kExpectedFrames, kInactiveFrames);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreeTimerMmioItemTest, MultipleInvalidFramesAreIgnored) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  // While there are multiple frames in the DTB, there is only one that meets all the requirements,
  // this will spew some logs about invalid frames being skipped, and if there is at least one valid
  // frame emit the ZBI item.
  auto kExpectedFrames = std::to_array<ExpectedFrame>({
      ExpectedFrame{
          .id = 1,
          .mmio_phys_el1 = 0x12341000,
          // While is available it is zero-size, so its ignored.
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 40,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
          .virt_irq =
              IrqInfo{
                  .irq = 39,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
      {
          .id = 7,
          .mmio_phys_el1 = 0x12346000,
          .mmio_phys_el0 = std::nullopt,
          .phys_irq =
              {
                  .irq = 44,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
  });

  auto kInactiveFrames = std::to_array<ExpectedFrame>({
      {
          .id = 3,
          .mmio_phys_el1 = 0x12347000,
          .mmio_phys_el0 = 0x12348000,
          .phys_irq =
              {
                  .irq = 45,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },

      {
          .id = 0,
          .mmio_phys_el1 = 0x12349000,
          .mmio_phys_el0 = 0,
          .phys_irq =
              {
                  .irq = 46,
                  .irq_flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                               ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
              },
      },
  });

  auto fdt = timer_invalid_frames();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerMmioItem> shim("test", fdt);

  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_KERNEL_DRIVER &&
        header->extra == ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO) {
      present = true;
      ASSERT_GE(payload.size_bytes(), sizeof(zbi_dcfg_arm_generic_timer_mmio_driver_t));
      const auto* item =
          reinterpret_cast<const zbi_dcfg_arm_generic_timer_mmio_driver_t*>(payload.data());
      CheckGenericTimerMmioItem(*item, 0x12340000, std::nullopt, kExpectedFrames, kInactiveFrames);
    }
  }
  ASSERT_TRUE(present);
}

}  // namespace
