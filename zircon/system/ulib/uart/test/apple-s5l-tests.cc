// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/uart/apple-s5l.h>
#include <lib/uart/mock.h>

#include <zxtest/zxtest.h>

namespace {

struct TestSyncPolicy : uart::UnsynchronizedPolicy {
  struct Waiter : uart::UnsynchronizedPolicy::Waiter {
    bool Wake() { return true; }
  };
};

using TestDriver = uart::KernelDriver<uart::apple_s5l::Driver, uart::mock::IoProvider,
                                      TestSyncPolicy, uart::mock::IrqProvider>;
constexpr zbi_dcfg_simple_t kConfig = {};

TEST(AppleS5lTests, InitPreservesClockAndMasksInterrupts) {
  TestDriver driver(kConfig);
  driver.io().mock().ExpectRead(uint32_t{0xb20f}, 0x04).ExpectWrite(uint32_t{0x8005}, 0x04);
  driver.Init();
}

TEST(AppleS5lTests, WritePollsAndConvertsNewline) {
  TestDriver driver(kConfig);
  driver.io()
      .mock()
      .ExpectRead(uint32_t{0}, 0x10)
      .ExpectRead(uint32_t{2}, 0x10)
      .ExpectWrite(uint32_t{'a'}, 0x20)
      .ExpectRead(uint32_t{2}, 0x10)
      .ExpectWrite(uint32_t{'\r'}, 0x20)
      .ExpectRead(uint32_t{2}, 0x10)
      .ExpectWrite(uint32_t{'\n'}, 0x20);
  EXPECT_EQ(2, driver.Write("a\n"));
}

TEST(AppleS5lTests, ReadOnlyConsumesAvailableData) {
  TestDriver driver(kConfig);
  driver.io()
      .mock()
      .ExpectRead(uint32_t{0}, 0x10)
      .ExpectRead(uint32_t{1}, 0x10)
      .ExpectRead(uint32_t{'q'}, 0x24)
      .ExpectRead(uint32_t{0}, 0x10);
  EXPECT_FALSE(driver.Read().has_value());
  EXPECT_EQ(static_cast<uint8_t>('q'), driver.Read());
  EXPECT_FALSE(driver.Read().has_value());
}

TEST(AppleS5lTests, MatchesOnlyItsZbiDriverType) {
  zbi_header_t header = {
      .type = ZBI_TYPE_KERNEL_DRIVER,
      .length = sizeof(kConfig),
      .extra = ZBI_KERNEL_DRIVER_APPLE_S5L_UART,
      .flags = ZBI_FLAGS_VERSION,
      .reserved0 = 0,
      .reserved1 = 0,
      .magic = ZBI_ITEM_MAGIC,
      .crc32 = ZBI_ITEM_NO_CRC32,
  };
  EXPECT_TRUE(uart::apple_s5l::Driver::TryMatch(header, &kConfig).has_value());
  header.extra = ZBI_KERNEL_DRIVER_EXYNOS_USI_UART;
  EXPECT_FALSE(uart::apple_s5l::Driver::TryMatch(header, &kConfig).has_value());
  header.extra = ZBI_KERNEL_DRIVER_APPLE_S5L_UART;
  header.length = sizeof(kConfig) - 1;
  EXPECT_FALSE(uart::apple_s5l::Driver::TryMatch(header, &kConfig).has_value());
}

using Guard = uart::UnsynchronizedPolicy::Guard<uart::UnsynchronizedPolicy::DefaultLockPolicy>;

TEST(AppleS5lTests, LineControlPreservesUnspecifiedFields) {
  TestDriver driver(kConfig);
  driver.io().mock().ExpectRead<uint32_t>(0x83, 0).ExpectWrite<uint32_t>(0xae, 0);
  driver.SetLineControl(uart::DataBits::k7, uart::Parity::kEven, uart::StopBits::k2);
  driver.io().mock().ExpectRead<uint32_t>(0xae, 0).ExpectWrite<uint32_t>(0xa6, 0);
  driver.SetLineControl(std::nullopt, uart::Parity::kOdd, std::nullopt);
  driver.io().mock().ExpectRead<uint32_t>(0xa6, 0).ExpectWrite<uint32_t>(0xa6, 0);
  driver.SetLineControl(std::nullopt, std::nullopt, std::nullopt);
  driver.io().mock().ExpectRead<uint32_t>(0xa6, 0).ExpectWrite<uint32_t>(0x83, 0);
  driver.SetLineControl();
}

TEST(AppleS5lTests, InitInterruptAcknowledgesAndEnablesReceive) {
  TestDriver driver(kConfig);
  driver.io()
      .mock()
      .ExpectWrite<uint32_t>(0x230, 0x10)
      .ExpectRead<uint32_t>(5, 4)
      .ExpectWrite<uint32_t>(0x1205, 4);
  driver.InitInterrupt();
  EXPECT_TRUE(driver.irq().enabled());
  EXPECT_EQ(driver.irq().init_count(), 1u);
}

TEST(AppleS5lTests, ReceiveTimeoutDrainsAndAcknowledgesOnlyFlags) {
  TestDriver driver(kConfig);
  driver.io()
      .mock()
      .ExpectRead<uint32_t>(0x207, 0x10)
      .ExpectWrite<uint32_t>(0x200, 0x10)
      .ExpectRead<uint32_t>('a', 0x24)
      .ExpectRead<uint32_t>(7, 0x10)
      .ExpectRead<uint32_t>('b', 0x24)
      .ExpectRead<uint32_t>(6, 0x10);
  unsigned count = 0;
  driver.Interrupt([](auto&) { FAIL("Unexpected TX interrupt"); },
                   [&](auto& irq) {
                     Guard guard(&irq.lock(), SOURCE_TAG);
                     EXPECT_EQ(irq.ReadChar(), static_cast<uint32_t>('a' + count++));
                   });
  EXPECT_EQ(count, 2u);
}

TEST(AppleS5lTests, FullReceiveQueueDoesNotConsumeCharacter) {
  TestDriver driver(kConfig);
  driver.io()
      .mock()
      .ExpectRead<uint32_t>(0x211, 0x10)
      .ExpectWrite<uint32_t>(0x210, 0x10)
      .ExpectRead<uint32_t>(0x3205, 4)
      .ExpectWrite<uint32_t>(0x2005, 4);
  driver.Interrupt([](auto&) { FAIL("Unexpected TX interrupt"); },
                   [](auto& irq) {
                     Guard guard(&irq.lock(), SOURCE_TAG);
                     irq.DisableInterrupt();
                   });
  driver.io().mock().ExpectRead<uint32_t>(0x2005, 4).ExpectWrite<uint32_t>(0x3205, 4);
  driver.EnableRxInterrupt();
}

TEST(AppleS5lTests, TransmitReadySurvivesReceiveAcknowledgement) {
  TestDriver driver(kConfig);
  driver.io()
      .mock()
      .ExpectRead<uint32_t>(0x237, 0x10)
      .ExpectWrite<uint32_t>(0x230, 0x10)
      .ExpectRead<uint32_t>('q', 0x24)
      .ExpectRead<uint32_t>(6, 0x10)
      .ExpectRead<uint32_t>(0x3205, 4)
      .ExpectWrite<uint32_t>(0x1205, 4);
  bool notified = false;
  driver.Interrupt(
      [&](auto& irq) {
        {
          Guard guard(&irq.lock(), SOURCE_TAG);
          irq.DisableInterrupt();
        }
        notified = irq.Notify();
      },
      [](auto& irq) {
        Guard guard(&irq.lock(), SOURCE_TAG);
        EXPECT_EQ(irq.ReadChar(), static_cast<uint32_t>('q'));
      });
  EXPECT_TRUE(notified);
}

TEST(AppleS5lTests, SpuriousInterruptDoesNotDeliverData) {
  TestDriver driver(kConfig);
  driver.io().mock().ExpectRead<uint32_t>(7, 0x10).ExpectWrite<uint32_t>(0, 0x10);
  driver.Interrupt([](auto&) { FAIL("Unexpected TX interrupt"); },
                   [](auto&) { FAIL("Unexpected RX interrupt"); });
}

}  // namespace
