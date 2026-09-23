// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/uart/mock.h>
#include <lib/uart/ns8250.h>
#include <lib/uart/uart.h>

#include <zxtest/zxtest.h>

namespace {

using SimpleTestDriver = uart::KernelDriver<uart::ns8250::PxaDriver, uart::mock::IoProvider,
                                            uart::UnsynchronizedPolicy, uart::mock::IrqProvider>;
constexpr zbi_dcfg_simple_t kTestConfig = {};

TEST(PXAtests, HelloWorld) {
  SimpleTestDriver driver(kTestConfig);

  driver.io()
      .mock()
      // Init()
      .ExpectWrite(uint8_t{0b0100'0000}, 1)  // IER
      .ExpectWrite(uint8_t{0b0000'1111}, 2)  // FCR
      .ExpectWrite(uint8_t{0b0000'0011}, 4)  // MCR
      .ExpectRead(uint8_t{0b1110'0001}, 2)   // IIR
      // Write()
      .ExpectRead(uint8_t{0b0110'0000}, 5)  // TxReady -> true
      .ExpectWrite(uint8_t{'h'}, 0)         // Write
      .ExpectWrite(uint8_t{'i'}, 0)
      .ExpectWrite(uint8_t{'\r'}, 0)
      .ExpectWrite(uint8_t{'\n'}, 0);

  driver.Init();
  EXPECT_EQ(3, driver.Write("hi\n"));
}

TEST(PXAtests, SetLineControl8N1) {
  SimpleTestDriver driver(kTestConfig);

  driver.io()
      .mock()
      // Init()
      .ExpectWrite(uint8_t{0b0100'0000}, 1)  // IER
      .ExpectWrite(uint8_t{0b0000'1111}, 2)  // FCR
      .ExpectWrite(uint8_t{0b0000'0011}, 4)  // MCR
      .ExpectRead(uint8_t{0b1110'0001}, 2)   // IIR
      // SetLineControl()
      .ExpectWrite(uint8_t{0b1000'0000}, 3)
      .ExpectWrite(uint8_t{0b0000'0001}, 0)
      .ExpectWrite(uint8_t{0b0000'0000}, 1)
      .ExpectWrite(uint8_t{0b0000'0011}, 3);

  driver.Init();
  driver.SetLineControl(uart::DataBits::k8, uart::Parity::kNone, uart::StopBits::k1);
}

TEST(PXAtests, SetLineControl7E1) {
  SimpleTestDriver driver(kTestConfig);

  driver.io()
      .mock()
      // Init()
      .ExpectWrite(uint8_t{0b0100'0000}, 1)  // IER
      .ExpectWrite(uint8_t{0b0000'1111}, 2)  // FCR
      .ExpectWrite(uint8_t{0b0000'0011}, 4)  // MCR
      .ExpectRead(uint8_t{0b1110'0001}, 2)   // IIR
      // SetLineControl()
      .ExpectWrite(uint8_t{0b1000'0000}, 3)
      .ExpectWrite(uint8_t{0b0000'0001}, 0)
      .ExpectWrite(uint8_t{0b0000'0000}, 1)
      .ExpectWrite(uint8_t{0b0001'1010}, 3);

  driver.Init();
  driver.SetLineControl(uart::DataBits::k7, uart::Parity::kEven, uart::StopBits::k1);
}

TEST(PXAtests, Read) {
  SimpleTestDriver driver(kTestConfig);

  driver.io()
      .mock()
      // Init()
      .ExpectWrite(uint8_t{0b0100'0000}, 1)  // IER
      .ExpectWrite(uint8_t{0b0000'1111}, 2)  // FCR
      .ExpectWrite(uint8_t{0b0000'0011}, 4)  // MCR
      .ExpectRead(uint8_t{0b1110'0001}, 2)   // IIR
      // Write()
      .ExpectRead(uint8_t{0b0110'0000}, 5)  // TxReady -> true
      .ExpectWrite(uint8_t{'?'}, 0)         // Write
      .ExpectWrite(uint8_t{'\r'}, 0)
      .ExpectWrite(uint8_t{'\n'}, 0)
      // Read()
      .ExpectRead(uint8_t{0b0110'0001}, 5)  // Read (data_ready)
      .ExpectRead(uint8_t{'q'}, 0)          // Read (data)
      // Read()
      .ExpectRead(uint8_t{0b0110'0001}, 5)  // Read (data_ready)
      .ExpectRead(uint8_t{'\r'}, 0);        // Read (data)

  driver.Init();
  EXPECT_EQ(2, driver.Write("?\n"));
  EXPECT_EQ(uint8_t{'q'}, driver.Read());
  EXPECT_EQ(uint8_t{'\r'}, driver.Read());
}

}  // namespace
