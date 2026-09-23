// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "uart16550.h"

#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>

#include <zxtest/zxtest.h>

namespace {

class Uart16550Harness : public zxtest::Test {
 public:
  void SetUp() override {
    zx::interrupt interrupt;
    ASSERT_OK(zx::interrupt::create({}, 0, ZX_INTERRUPT_VIRTUAL, &interrupt));

    port_mock_
        .ExpectWrite<uint8_t>(0b1000'0000, 3)   // divisor latch enable
        .ExpectWrite<uint8_t>(0b1110'0111, 2)   // fifo control reset
        .ExpectWrite<uint8_t>(0b0000'0000, 3)   // divisor latch disable
        .ExpectRead<uint8_t>(0b1110'0000, 2)    // interrupt identify
        .ExpectRead<uint8_t>(0b0000'0000, 3)    // line control
        .ExpectWrite<uint8_t>(0b1000'0000, 3)   // divisor latch enable
        .ExpectWrite<uint8_t>(0b0000'0001, 0)   // lower
        .ExpectWrite<uint8_t>(0b0000'0000, 1)   // upper
        .ExpectWrite<uint8_t>(0b0000'0011, 3)   // 8N1
        .ExpectWrite<uint8_t>(0b0000'1011, 4)   // no flow control
        .ExpectWrite<uint8_t>(0b1000'0000, 3)   // divisor latch enable
        .ExpectWrite<uint8_t>(0b1110'0111, 2)   // fifo control reset
        .ExpectWrite<uint8_t>(0b0000'0000, 3)   // divisor latch disable
        .ExpectWrite<uint8_t>(0b0000'1101, 1);  // enable interrupts

    device_ = std::make_unique<uart16550::Uart16550>();
    ASSERT_OK(device_->Init(std::move(interrupt), *port_mock_.io()));
    ASSERT_EQ(device_->FifoDepth(), 64);
    ASSERT_FALSE(device_->Enabled());

    ASSERT_FALSE(device_->Enabled());

    auto [client, server] = fdf::Endpoints<fuchsia_hardware_serialimpl::Device>::Create();
    device_->GetHandler()(std::move(server));
    fdf::WireClient<fuchsia_hardware_serialimpl::Device> driver_client(
        std::move(client), fdf::Dispatcher::GetCurrent()->get());

    fdf::Arena arena('TEST');
    driver_client.buffer(arena)->Enable(true).ThenExactlyOnce([this](auto& result) {
      ASSERT_TRUE(result.ok());
      EXPECT_TRUE(result->is_ok());
      runtime_.Quit();
    });
    runtime_.Run();
    runtime_.ResetQuit();

    ASSERT_TRUE(device_->Enabled());

    port_mock_.VerifyAndClear();

    auto result = driver_client.UnbindMaybeGetEndpoint();
    ASSERT_TRUE(result.is_ok());
    driver_client_end_ = *std::move(result);
  }

  void TearDown() override {
    if (device_->Enabled()) {
      port_mock_.ExpectWrite<uint8_t>(0b0000'0000, 1);  // disable interrupts
    } else {
      port_mock_.ExpectNoIo();
    }

    uart16550::Uart16550* device = device_.release();
    device->DdkRelease();

    // Verify and clear after joining with the interrupt thread.
    port_mock_.VerifyAndClear();
    port_mock_.ExpectNoIo();
  }

  uart16550::Uart16550& Device() { return *device_; }

  hwreg::Mock& PortMock() { return port_mock_; }

  fdf_testing::DriverRuntime& Runtime() { return runtime_; }

  void InterruptDriver() { ASSERT_OK(Device().InterruptHandle()->trigger(0, zx::time_boot())); }

  fdf::WireClient<fuchsia_hardware_serialimpl::Device> CreateDriverClient() {
    return fdf::WireClient<fuchsia_hardware_serialimpl::Device>(
        std::move(driver_client_end_), fdf::Dispatcher::GetCurrent()->get());
  }

 private:
  fdf_testing::DriverRuntime runtime_;
  hwreg::Mock port_mock_;
  std::unique_ptr<uart16550::Uart16550> device_;
  fdf::ClientEnd<fuchsia_hardware_serialimpl::Device> driver_client_end_;
};

TEST_F(Uart16550Harness, GetInfo) {
  PortMock().ExpectNoIo();

  fdf::WireClient client = CreateDriverClient();
  fdf::Arena arena('TEST');
  client.buffer(arena)->GetInfo().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    Runtime().Quit();
  });
  Runtime().Run();
}

TEST_F(Uart16550Harness, Config) {
  PortMock()
      .ExpectWrite<uint8_t>(0b0000'0000, 1)   // disable interrupts
      .ExpectRead<uint8_t>(0b0000'0000, 3)    // line control
      .ExpectWrite<uint8_t>(0b1000'0000, 3)   // enable divisor latch
      .ExpectWrite<uint8_t>(0b1000'0000, 0)   // lower
      .ExpectWrite<uint8_t>(0b0001'0110, 1)   // upper
      .ExpectWrite<uint8_t>(0b0001'1101, 3)   // 6E2
      .ExpectWrite<uint8_t>(0b0010'1000, 4)   // automatic flow control
      .ExpectRead<uint8_t>(0b0001'1101, 3)    // line control
      .ExpectWrite<uint8_t>(0b1001'1101, 3)   // enable divisor latch
      .ExpectWrite<uint8_t>(0b0100'0000, 0)   // lower
      .ExpectWrite<uint8_t>(0b0000'1011, 1)   // upper
      .ExpectWrite<uint8_t>(0b0001'1101, 3);  // disable divisor latch

  fdf::WireClient client = CreateDriverClient();

  fdf::Arena arena('TEST');
  client.buffer(arena)->Enable(false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  static constexpr uint32_t serial_test_config =
      fuchsia_hardware_serialimpl::wire::kSerialDataBits6 |
      fuchsia_hardware_serialimpl::wire::kSerialStopBits2 |
      fuchsia_hardware_serialimpl::wire::kSerialParityEven |
      fuchsia_hardware_serialimpl::wire::kSerialFlowCtrlCtsRts;

  client.buffer(arena)->Config(20, serial_test_config).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  client.buffer(arena)
      ->Config(40, fuchsia_hardware_serialimpl::wire::kSerialSetBaudRateOnly)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });

  client.buffer(arena)->Config(0, serial_test_config).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  });
  client.buffer(arena)->Config(UINT32_MAX, serial_test_config).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  });
  client.buffer(arena)->Config(1, serial_test_config).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
    Runtime().Quit();
  });

  Runtime().Run();
}

TEST_F(Uart16550Harness, Enable) {
  PortMock()
      .ExpectWrite<uint8_t>(0b0000'0000, 1)   // disable interrupts
      .ExpectWrite<uint8_t>(0b1000'0000, 3)   // divisor latch enable
      .ExpectWrite<uint8_t>(0b1110'0111, 2)   // fifo control reset
      .ExpectWrite<uint8_t>(0b0000'0000, 3)   // divisor latch disable
      .ExpectWrite<uint8_t>(0b0000'1101, 1);  // enable interrupts

  fdf::WireClient client = CreateDriverClient();

  fdf::Arena arena('TEST');
  client.buffer(arena)->Enable(false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    ASSERT_FALSE(Device().Enabled());
  });

  client.buffer(arena)->Enable(false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    ASSERT_FALSE(Device().Enabled());
  });

  client.buffer(arena)->Enable(true).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    ASSERT_TRUE(Device().Enabled());
    Runtime().Quit();
  });

  Runtime().Run();
}

TEST_F(Uart16550Harness, Read) {
  PortMock()
      .ExpectWrite<uint8_t>(0b0000'0000, 1)  // disable interrupts
      .ExpectWrite<uint8_t>(0b1000'0000, 3)  // divisor latch enable
      .ExpectWrite<uint8_t>(0b1110'0111, 2)  // fifo control reset
      .ExpectWrite<uint8_t>(0b0000'0000, 3)  // divisor latch disable
      .ExpectWrite<uint8_t>(0b0000'1101, 1)  // enable interrupts
      .ExpectRead<uint8_t>(0b0000'0000, 5)   // data not ready
      .ExpectRead<uint8_t>(0b0000'0100, 2)   // rx available interrupt id
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0x0F, 0)          // buffer[0]
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0xF0, 0)          // buffer[1]
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0x59, 0)          // buffer[2]
      .ExpectRead<uint8_t>(0b0000'0000, 5);  // data ready

  fdf::WireClient client = CreateDriverClient();

  fdf::Arena arena('TEST');
  client.buffer(arena)->Enable(false).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  client.buffer(arena)->Read().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_error());
    EXPECT_EQ(result->error_value(), ZX_ERR_BAD_STATE);
  });

  client.buffer(arena)->Enable(true).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  const std::vector<uint8_t> expect_buffer{0x0F, 0xF0, 0x59};

  client.buffer(arena)->Read().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());

    const std::vector actual_buffer(result->value()->data.cbegin(), result->value()->data.cend());
    EXPECT_EQ(expect_buffer, actual_buffer);

    Runtime().Quit();
  });

  // Run the dispatcher so that driver processes the read request.
  Runtime().RunUntilIdle();

  // The RX FIFO was empty, so the driver should have stored the request. Interrupt the driver to
  // make it read from the FIFO and complete the request.
  InterruptDriver();

  Runtime().Run();
}

TEST_F(Uart16550Harness, Write) {
  PortMock()
      .ExpectWrite<uint8_t>(0b0000'0000, 1)   // disable interrupts
      .ExpectWrite<uint8_t>(0b1000'0000, 3)   // divisor latch enable
      .ExpectWrite<uint8_t>(0b1110'0111, 2)   // fifo control reset
      .ExpectWrite<uint8_t>(0b0000'0000, 3)   // divisor latch disable
      .ExpectWrite<uint8_t>(0b0000'1101, 1)   // enable interrupts
      .ExpectRead<uint8_t>(0b0000'1101, 1)    // read interrupts
      .ExpectWrite<uint8_t>(0b0000'1111, 1)   // write interrupts
      .ExpectRead<uint8_t>(0b0100'0000, 5)    // tx empty
      .ExpectWrite<uint8_t>(0xDE, 0)          // writable_buffer[0]
      .ExpectWrite<uint8_t>(0xAD, 0)          // writable_buffer[1]
      .ExpectWrite<uint8_t>(0xBE, 0)          // writable_buffer[2]
      .ExpectWrite<uint8_t>(0xEF, 0)          // writable_buffer[3]
      .ExpectRead<uint8_t>(0b0000'0010, 2)    // tx empty interrupt id
      .ExpectRead<uint8_t>(0b0000'1111, 1)    // read interrupts
      .ExpectWrite<uint8_t>(0b0000'1101, 1);  // write interrupts

  fdf::WireClient client = CreateDriverClient();

  fdf::Arena arena('TEST');
  client.buffer(arena)->Enable(false).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  client.buffer(arena)
      ->Write(fidl::VectorView<uint8_t>(arena, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_BAD_STATE);
      });

  client.buffer(arena)->Enable(true).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  uint8_t writable_buffer[4] = {0xDE, 0xAD, 0xBE, 0xEF};
  client.buffer(arena)
      ->Write(fidl::VectorView<uint8_t>::FromExternal(writable_buffer, std::size(writable_buffer)))
      .ThenExactlyOnce([&](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
        Runtime().Quit();
      });

  // Run the dispatcher so that driver processes the write request.
  Runtime().RunUntilIdle();

  // The driver should have written the data to the TX FIFO by this point. Raise a TX FIFO empty
  // interrupt to make the driver complete the request.
  InterruptDriver();

  Runtime().Run();
}

TEST_F(Uart16550Harness, ReadDataInFifo) {
  PortMock()
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0x0F, 0)          // buffer[0]
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0xF0, 0)          // buffer[1]
      .ExpectRead<uint8_t>(0b0000'0001, 5)   // data ready
      .ExpectRead<uint8_t>(0x59, 0)          // buffer[2]
      .ExpectRead<uint8_t>(0b0000'0000, 5);  // data ready

  fdf::WireClient client = CreateDriverClient();

  const std::vector<uint8_t> expect_buffer{0x0F, 0xF0, 0x59};

  fdf::Arena arena('TEST');
  client.buffer(arena)->Read().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());

    const std::vector actual_buffer(result->value()->data.cbegin(), result->value()->data.cend());
    EXPECT_EQ(expect_buffer, actual_buffer);
  });

  // There is already data in the FIFO, so our read request should be completed immediately without
  // involvement from the interrupt handler.
  Runtime().RunUntilIdle();
}

}  // namespace
