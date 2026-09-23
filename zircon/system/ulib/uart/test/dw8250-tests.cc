// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/uart/dw8250.h>
#include <lib/uart/mock.h>
#include <lib/uart/uart.h>

#include <zxtest/zxtest.h>

namespace {

using SimpleTestDriver = uart::KernelDriver<uart::dw8250::Driver, uart::mock::IoProvider,
                                            uart::UnsynchronizedPolicy, uart::mock::IrqProvider>;
constexpr zbi_dcfg_simple_t kTestConfig = {};

template <typename Mock>
void AppendInitSequence(Mock& mock) {
  mock
      // Init()
      .ExpectRead(uint32_t{0x103d32}, 0x3d)  // ComponentParameterRegister read
                                             // From real hardware:
                                             // 0x103d32 =
                                             //   256 byte fifo
                                             //   DMA_EXTRA
                                             //   UART_ADD_ENCODED_PARAMS
                                             //   SHADOW
                                             //   FIFO_STAT
                                             //   ADDITIONAL_FAT
                                             //   THRE_MODE
                                             //   AFCE_MODE
                                             //   APB_DATA_WIDTH = 2

      .ExpectWrite(uint32_t{0x80}, 1)  // InterruptEnableRegister write
      .ExpectWrite(uint32_t{0x17}, 2)  // FifoControlRegister write
      .ExpectWrite(uint32_t{0x03}, 4)  // ModemControlRegister write
      // End of Init()
      ;
}

TEST(Dw8250Tests, HelloWorld) {
  SimpleTestDriver driver(kTestConfig);

  AppendInitSequence(driver.io().mock());
  driver.io()
      .mock()
      // Write()
      .ExpectRead(uint32_t{0b000'0010}, 31)  // UserStatus.TFNF (tx fifo not full)
      .ExpectRead(uint32_t{0x100}, 32)       // TransmitFifoLevel (reads 256)
      .ExpectWrite(uint32_t{'h'}, 0)         // Write
      .ExpectWrite(uint32_t{'i'}, 0)
      .ExpectWrite(uint32_t{'\r'}, 0)
      .ExpectWrite(uint32_t{'\n'}, 0);

  driver.Init();
  EXPECT_EQ(3, driver.Write("hi\n"));
}

TEST(Dw8250Tests, SetLineControl8N1) {
  SimpleTestDriver driver(kTestConfig);

  AppendInitSequence(driver.io().mock());
  driver.io()
      .mock()
      // SetLineControl()
      .ExpectRead(uint32_t{0b0000'0000}, 31)  // UserStatus
      .ExpectWrite(uint32_t{0b1000'0000}, 3)  // LineControl setting divisor latch access
      .ExpectWrite(uint32_t{0b0000'0001}, 0)  // Divisor to 1
      .ExpectWrite(uint32_t{0b0000'0000}, 1)
      .ExpectRead(uint32_t{0b0000'0000}, 31)   // UserStatus
      .ExpectWrite(uint32_t{0b0000'0011}, 3);  // LineControl setting 8N1

  driver.Init();
  driver.SetLineControl(uart::DataBits::k8, uart::Parity::kNone, uart::StopBits::k1);
}

TEST(Dw8250Tests, SetLineControl7E1) {
  SimpleTestDriver driver(kTestConfig);

  AppendInitSequence(driver.io().mock());
  driver.io()
      .mock()
      // SetLineControl()
      .ExpectRead(uint32_t{0b0000'0000}, 31)  // UserStatus
      .ExpectWrite(uint32_t{0b1000'0000}, 3)  // LineControl setting divisor latch access
      .ExpectWrite(uint32_t{0b0000'0001}, 0)  // Divisor to 1
      .ExpectWrite(uint32_t{0b0000'0000}, 1)
      .ExpectRead(uint32_t{0b0000'0000}, 31)   // UserStatus
      .ExpectWrite(uint32_t{0b0001'1010}, 3);  // LineControl setting 7E1

  driver.Init();
  driver.SetLineControl(uart::DataBits::k7, uart::Parity::kEven, uart::StopBits::k1);
}

TEST(Dw8250Tests, Write) {
  SimpleTestDriver driver(kTestConfig);

  // Write using the expected FIFO_STAT mode
  AppendInitSequence(driver.io().mock());
  driver.io()
      .mock()
      // Write()
      .ExpectRead(uint32_t{0b000'0000}, 31)  // UserStatus.TFNF (tx fifo full)
      // TX fifo is full, poll until it isn't.
      .ExpectRead(uint32_t{0b000'0010}, 31)  // UserStatus.TFNF (tx fifo not full)
      .ExpectRead(uint32_t{0x100 - 2}, 32)   // TransmitFifoLevel (reads 256 - 2)
      // Only 2 bytes available in the tx fifo.
      .ExpectWrite(uint32_t{'a'}, 0)  // Write
      .ExpectWrite(uint32_t{'b'}, 0)
      // Go back to polling if the fifo has space in it.
      .ExpectRead(uint32_t{0b000'0000}, 31)  // UserStatus.TFNF (tx fifo full)
      // TX fifo is full, poll until it isn't.
      .ExpectRead(uint32_t{0b000'0010}, 31)  // UserStatus.TFNF (tx fifo not full)
      .ExpectRead(uint32_t{0x100}, 32)       // TransmitFifoLevel (reads 256)
      // Write the rest of the message.
      .ExpectWrite(uint32_t{'c'}, 0)
      .ExpectWrite(uint32_t{'d'}, 0)
      .ExpectWrite(uint32_t{'e'}, 0)
      .ExpectWrite(uint32_t{'f'}, 0);

  driver.Init();
  EXPECT_EQ(6, driver.Write("abcdef"));
}

TEST(Dw8250Tests, Read) {
  SimpleTestDriver driver(kTestConfig);

  AppendInitSequence(driver.io().mock());
  driver.io()
      .mock()
      // Write()
      .ExpectRead(uint32_t{0b000'0010}, 31)  // UserStatus.TFNF (tx fifo not full)
      .ExpectRead(uint32_t{0x100}, 32)       // TransmitFifoLevel (reads 256)
      .ExpectWrite(uint32_t{'?'}, 0)         // Write
      .ExpectWrite(uint32_t{'\r'}, 0)
      .ExpectWrite(uint32_t{'\n'}, 0)
      // Read()
      .ExpectRead(uint32_t{0b0110'0001}, 5)  // Read (data_ready)
      .ExpectRead(uint32_t{'q'}, 0)          // Read (data)
      // Read()
      .ExpectRead(uint32_t{0b0110'0001}, 5)  // Read (data_ready)
      .ExpectRead(uint32_t{'\n'}, 0);        // Read (data)

  driver.Init();
  EXPECT_EQ(2, driver.Write("?\n"));
  EXPECT_EQ(uint32_t{'q'}, driver.Read());
  EXPECT_EQ(uint32_t{'\n'}, driver.Read());
}

}  // namespace
