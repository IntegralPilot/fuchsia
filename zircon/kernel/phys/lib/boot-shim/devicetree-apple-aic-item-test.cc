// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/boot-shim/apple-aic.h>
#include <lib/boot-shim/apple-platform.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>

#include <zxtest/zxtest.h>

namespace {
TEST(AppleDevicetree, PlatformIdentity) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic.dtb");
  ASSERT_TRUE(dt.is_ok());
  boot_shim::DevicetreeBootShim<boot_shim::AppleDevicetreePlatformItem> shim("test", dt->fdt());
  ASSERT_TRUE(shim.Init());
  auto id = shim.Get<boot_shim::AppleDevicetreePlatformItem>().payload();
  ASSERT_TRUE(id);
  EXPECT_EQ(id->vid, 0u);
  EXPECT_EQ(id->pid, 0u);
  EXPECT_STREQ(id->board_name, "apple-t8122");
}

TEST(AppleDevicetree, PlatformIdentityRequiresRootCompatible) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic_bad_event.dtb");
  ASSERT_TRUE(dt.is_ok());
  boot_shim::DevicetreeBootShim<boot_shim::AppleDevicetreePlatformItem> shim("test", dt->fdt());
  ASSERT_TRUE(shim.Init());
  EXPECT_FALSE(shim.Get<boot_shim::AppleDevicetreePlatformItem>().payload());
}

TEST(AppleDevicetree, CoreAndEventRanges) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic.dtb");
  ASSERT_TRUE(dt.is_ok());
  boot_shim::DevicetreeBootShim<boot_shim::AppleDevicetreeAic3Item> shim("test", dt->fdt());
  unsigned observed = 0;
  shim.set_mmio_observer([&](const auto& range) {
    EXPECT_EQ(range.address, 0x2d1000000u);
    EXPECT_EQ(range.size, 0x184000u);
    ++observed;
  });
  ASSERT_TRUE(shim.Init());
  auto config = shim.Get<boot_shim::AppleDevicetreeAic3Item>().payload();
  ASSERT_TRUE(config);
  EXPECT_EQ(config->mmio_phys, 0x2d1000000u);
  EXPECT_EQ(config->mmio_size, 0x184000u);
  EXPECT_EQ(config->event_offset, 0x40000u);
  EXPECT_EQ(observed, 1u);
}

TEST(AppleDevicetree, RejectsEventOutsideCoreRange) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic_bad_event.dtb");
  ASSERT_TRUE(dt.is_ok());
  boot_shim::DevicetreeBootShim<boot_shim::AppleDevicetreeAic3Item> shim("test", dt->fdt());
  ASSERT_TRUE(shim.Init());
  EXPECT_FALSE(shim.Get<boot_shim::AppleDevicetreeAic3Item>().payload());
}

#ifdef EXPERIMENTAL_APPLE
TEST(AppleDevicetree, NamedTimersUseAppleFiqNumbers) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic.dtb");
  ASSERT_TRUE(dt.is_ok());
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreeTimerItem> shim("test", dt->fdt());
  ASSERT_TRUE(shim.Init());
  auto config = shim.Get<boot_shim::ArmDevicetreeTimerItem>().payload();
  ASSERT_TRUE(config);
  EXPECT_EQ(config->irq_phys, 1u);
  EXPECT_EQ(config->irq_virt, 2u);
  EXPECT_EQ(config->irq_sphys, 0u);
  EXPECT_EQ(config->freq_override, 0u);
}
TEST(AppleDevicetree, UartInterruptUsesExternalVectorNamespace) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic.dtb");
  ASSERT_TRUE(dt.is_ok());
  using Drivers = std::variant<uart::null::Driver, uart::apple_s5l::Driver>;
  boot_shim::DevicetreeChosenNodeMatcher<Drivers> matcher("test", stdout);
  ASSERT_TRUE(devicetree::Match(dt->fdt(), matcher));
  auto config = matcher.uart_config();
  ASSERT_TRUE(config);
  config->Visit([]<typename Driver>(const uart::Config<Driver>& uart) {
    if constexpr (std::is_same_v<Driver, uart::apple_s5l::Driver>) {
      EXPECT_EQ(uart->irq, 760u);
      EXPECT_EQ(uart->flags, ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                 ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH);
    } else {
      FAIL("Unexpected UART driver");
    }
  });
}

TEST(AppleDevicetree, RejectsUnsupportedUartInterrupt) {
  auto dt = boot_shim::testing::LoadDtb("apple_aic_bad_irq.dtb");
  ASSERT_TRUE(dt.is_ok());
  using Drivers = std::variant<uart::null::Driver, uart::apple_s5l::Driver>;
  boot_shim::DevicetreeChosenNodeMatcher<Drivers> matcher("test", stdout);
  ASSERT_TRUE(devicetree::Match(dt->fdt(), matcher));
  auto config = matcher.uart_config();
  ASSERT_TRUE(config);
  config->Visit([]<typename Driver>(const uart::Config<Driver>& uart) {
    if constexpr (std::is_same_v<Driver, uart::apple_s5l::Driver>) {
      EXPECT_EQ(uart->irq, 0u);
    } else {
      FAIL("Unexpected UART driver");
    }
  });
}
#endif
}  // namespace
