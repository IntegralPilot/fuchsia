// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/serial/drivers/dw-apb-uart/dw-apb-uart.h"

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.powerdomain/cpp/wire.h>
#include <fidl/fuchsia.hardware.reset/cpp/wire.h>
#include <fidl/fuchsia.hardware.serial/cpp/fidl.h>
#include <lib/driver/fake-clock/cpp/fake-clock.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/fake-powerdomain/cpp/fake-powerdomain.h>
#include <lib/driver/fake-reset/cpp/fake-reset.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fidl_driver/cpp/wire_client.h>
#include <lib/sync/completion.h>
#include <lib/syslog/cpp/macros.h>

#include <future>

#include <gtest/gtest.h>

#include "src/devices/serial/drivers/dw-apb-uart/tests/device-state.h"

namespace {

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    static const fuchsia_hardware_serial::SerialPortInfo kSerialPortInfo{{
        .serial_class = fuchsia_hardware_serial::Class::kGeneric,
        .serial_vid = 0,
        .serial_pid = 0,
    }};

    fdf_fake::FakePDev::Config config;
    config.irqs[0] = {};
    zx_status_t status =
        zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &config.irqs[0]);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    zx::interrupt signaller;
    status = config.irqs[0].duplicate(ZX_RIGHT_SAME_RIGHTS, &signaller);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    state_.set_irq_signaller(std::move(signaller));
    config.mmios[0] = state_.GetMmio();
    pdev_.SetConfig(std::move(config));
    pdev_.AddFidlMetadata(fuchsia_hardware_serial::SerialPortInfo::kSerializableName,
                          kSerialPortInfo);

    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    constexpr std::string_view kInstanceName = "pdev";
    zx::result add_service_result =
        to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
            pdev_.GetInstanceHandler(dispatcher), kInstanceName);
    ZX_ASSERT(add_service_result.is_ok());

    fake_clock_.set_rate(200000000);

    // Bind clocks
    auto add_clock_apb = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
        fake_clock_.CreateInstanceHandler(dispatcher), "apb_pclk");
    ZX_ASSERT(add_clock_apb.is_ok());
    auto add_clock_baud = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
        fake_clock_.CreateInstanceHandler(dispatcher), "baudclk");
    ZX_ASSERT(add_clock_baud.is_ok());

    // Bind reset
    auto add_reset = to_driver_vfs.AddService<fuchsia_hardware_reset::Service>(
        fake_reset_.CreateInstanceHandler(), "reset");
    ZX_ASSERT(add_reset.is_ok());

    // Bind powerdomain
    auto add_power = to_driver_vfs.AddService<fuchsia_hardware_powerdomain::Service>(
        fake_power_domain_.CreateInstanceHandler(), "power-domain");
    ZX_ASSERT(add_power.is_ok());

    return zx::ok();
  }

  DeviceState& device_state() { return state_; }
  fdf_fake::FakeClock& fake_clock() { return fake_clock_; }

 private:
  DeviceState state_;
  fdf_fake::FakePDev pdev_;
  fdf_fake::FakeClock fake_clock_;
  fdf_fake::FakeReset fake_reset_;
  fdf_fake::FakePowerDomain fake_power_domain_;
};

class DwApbUartTestConfig {
 public:
  using DriverType = serial::DwApbUartDriver;
  using EnvironmentType = Environment;
};

class DwApbUartHarness : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf::WireSyncClient<fuchsia_hardware_serialimpl::Device> CreateClient() {
    zx::result driver_connect_result =
        driver_test().Connect<fuchsia_hardware_serialimpl::Service::Device>("dw-apb-uart");
    if (driver_connect_result.is_error()) {
      return {};
    }
    return fdf::WireSyncClient(std::move(driver_connect_result.value()));
  }

  fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device> CreateAsyncClient() {
    zx::result driver_connect_result =
        driver_test().Connect<fuchsia_hardware_serialimpl::Service::Device>("dw-apb-uart");
    ZX_ASSERT(driver_connect_result.is_ok());
    if (!async_dispatcher_.has_value()) {
      async_dispatcher_.emplace(
          fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher());
    }
    return fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>(
        std::move(driver_connect_result.value()), (*async_dispatcher_)->get());
  }

  void AsyncEnable(fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>& client,
                   bool enable = true) {
    auto promise = std::make_shared<std::promise<void>>();
    auto future = promise->get_future();
    auto arena = std::make_shared<fdf::Arena>('ENAB');
    client.buffer(*arena)->Enable(enable).Then(
        [promise,
         arena](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Enable>& result) {
          promise->set_value();
        });
    future.wait();
  }

  void SyncBarrier(fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>& client) {
    auto promise = std::make_shared<std::promise<void>>();
    auto future = promise->get_future();
    auto arena = std::make_shared<fdf::Arena>('BARR');
    client.buffer(*arena)->GetInfo().Then(
        [promise,
         arena](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::GetInfo>& result) {
          promise->set_value();
        });
    future.wait();
  }

  std::future<std::vector<uint8_t>> AsyncRead(
      fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>& client) {
    auto promise = std::make_shared<std::promise<std::vector<uint8_t>>>();
    auto future = promise->get_future();
    auto arena = std::make_shared<fdf::Arena>('READ');
    client.buffer(*arena)->Read().Then(
        [promise,
         arena](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Read>& result) {
          if (!result.ok() || !result->is_ok()) {
            promise->set_value({});
            return;
          }
          auto data = result->value()->data;
          promise->set_value(std::vector<uint8_t>(data.begin(), data.end()));
        });
    return future;
  }

  std::future<std::vector<uint8_t>> QueueAsyncRead(
      fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>& client) {
    auto future = AsyncRead(client);
    SyncBarrier(client);
    return future;
  }

  std::future<zx_status_t> AsyncWrite(
      fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>& client,
      std::span<const uint8_t> data) {
    auto promise = std::make_shared<std::promise<zx_status_t>>();
    auto future = promise->get_future();
    auto arena = std::make_shared<fdf::Arena>('WRIT');
    client.buffer(*arena)
        ->Write(
            fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(data.data()), data.size()))
        .Then([promise,
               arena](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Write>& result) {
          if (!result.ok()) {
            promise->set_value(result.status());
          } else if (result->is_error()) {
            promise->set_value(result->error_value());
          } else {
            promise->set_value(ZX_OK);
          }
        });
    return future;
  }

  std::future<zx_status_t> QueueAsyncWrite(
      fdf::WireSharedClient<fuchsia_hardware_serialimpl::Device>& client,
      std::span<const uint8_t> data) {
    auto future = AsyncWrite(client, data);
    SyncBarrier(client);
    return future;
  }

  fdf_testing::BackgroundDriverTest<DwApbUartTestConfig>& driver_test() { return driver_test_; }

  template <typename F>
  void PollUntil(F&& criteria) {
    fdf_testing::DriverRuntime::GetInstance()->RunUntil(std::forward<F>(criteria));
  }

 private:
  fdf_testing::BackgroundDriverTest<DwApbUartTestConfig> driver_test_;
  std::optional<fdf::UnownedSynchronizedDispatcher> async_dispatcher_;
};

TEST_F(DwApbUartHarness, GetInfo) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');
  auto result = client.buffer(arena)->GetInfo();
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  const auto& info = result->value()->info;
  ASSERT_EQ(info.serial_class, fuchsia_hardware_serial::Class::kGeneric);
  ASSERT_EQ(info.serial_pid, 0u);
  ASSERT_EQ(info.serial_vid, 0u);
}

TEST_F(DwApbUartHarness, Config) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  static constexpr uint32_t serial_test_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto lcr = uart::dw8250::LineControlRegister::Get().FromValue(env.device_state().lcr());
    ASSERT_EQ(lcr.word_length(), 3u);    // 8 data bits
    ASSERT_EQ(lcr.stop_bits(), 0u);      // 1 stop bit
    ASSERT_EQ(lcr.parity_enable(), 0u);  // parity none

    // Divisor is 109 (0x6d) for 115200 baud on 200MHz clock.
    ASSERT_EQ(env.device_state().dll(), 109u);
    ASSERT_EQ(env.device_state().dlh(), 0u);
  });
}

TEST_F(DwApbUartHarness, ConfigWithDlf) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  // Configure environment to support DLF.
  driver_test().RunInEnvironmentTypeContext(
      [](Environment& env) { env.device_state().set_additional_feat(true); });

  static constexpr uint32_t serial_test_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto lcr = uart::dw8250::LineControlRegister::Get().FromValue(env.device_state().lcr());
    ASSERT_EQ(lcr.word_length(), 3u);    // 8 data bits
    ASSERT_EQ(lcr.stop_bits(), 0u);      // 1 stop bit
    ASSERT_EQ(lcr.parity_enable(), 0u);  // parity none

    // With DLF supported:
    // Divisor is 108 (0x6c), and DLF fractional part is 8.
    ASSERT_EQ(env.device_state().dll(), 108u);
    ASSERT_EQ(env.device_state().dlh(), 0u);
    ASSERT_EQ(env.device_state().dlf(), 8u);
  });
}

TEST_F(DwApbUartHarness, ConfigClockFailure) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  // Configure FakeClock to fail GetRate.
  driver_test().RunInEnvironmentTypeContext(
      [](Environment& env) { env.fake_clock().set_get_rate_result(zx::error(ZX_ERR_INTERNAL)); });

  static constexpr uint32_t serial_test_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  // The call should fail because the clock rate couldn't be retrieved.
  ASSERT_TRUE(result->is_error());
  ASSERT_EQ(result->error_value(), ZX_ERR_INTERNAL);
}

TEST_F(DwApbUartHarness, ConfigFlowControlNotSupported) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  static constexpr uint32_t serial_flow_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone |
                                                 fuchsia_hardware_serialimpl::kSerialFlowCtrlCtsRts;
  auto result = client.buffer(arena)->Config(115200, serial_flow_config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_error());
  ASSERT_EQ(result->error_value(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(DwApbUartHarness, Enable) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  {
    auto result = client.buffer(arena)->Enable(true);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
    ASSERT_TRUE(ier.rx_available());
  });

  {
    auto result = client.buffer(arena)->Enable(false);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
    ASSERT_FALSE(ier.rx_available());
    ASSERT_FALSE(ier.tx_empty());
  });
}

TEST_F(DwApbUartHarness, Read) {
  auto client = CreateClient();
  fdf::Arena arena('READ');

  {
    auto result = client.buffer(arena)->Enable(true);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  const uint8_t input_data[] = {1, 2, 3, 4};
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(input_data); });

  auto result = client.buffer(arena)->Read();
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());
  ASSERT_EQ(result->value()->data.size(), sizeof(input_data));
  ASSERT_EQ(memcmp(result->value()->data.data(), input_data, sizeof(input_data)), 0);
}

TEST_F(DwApbUartHarness, Write) {
  auto client = CreateClient();
  fdf::Arena arena('WRIT');

  {
    auto result = client.buffer(arena)->Enable(true);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  uint8_t output_data[] = {5, 6, 7, 8};
  auto result = client.buffer(arena)->Write(
      fidl::VectorView<uint8_t>::FromExternal(output_data, sizeof(output_data)));
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), sizeof(output_data));
    ASSERT_EQ(memcmp(tx_buf.data(), output_data, sizeof(output_data)), 0);
  });
}

TEST_F(DwApbUartHarness, RxInterruptGating) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  {
    auto result = client.buffer(arena)->Enable(true);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  // Inject DwApbUart::kRxBufferSize bytes to fill the ring buffer.
  std::vector<uint8_t> fill_data(serial::DwApbUart::kRxBufferSize, 0xAA);
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(fill_data); });

  // Wait for the RX interrupt to be disabled because the buffer is full.
  PollUntil([&]() {
    bool rx_disabled = false;
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
      rx_disabled = !ier.rx_available();
    });
    return rx_disabled;
  });

  // Verify that the RX interrupt is now disabled because the buffer is full.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
    ASSERT_FALSE(ier.rx_available());
  });

  // Now, read some data from the driver. This will consume up to serial::DwApbUart::kRxBufferSize
  // bytes.
  {
    auto result = client.buffer(arena)->Read();
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    ASSERT_EQ(result->value()->data.size(), serial::DwApbUart::kRxBufferSize);
  }

  // Verify that the RX interrupt has been re-enabled because the buffer is no longer full.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
    ASSERT_TRUE(ier.rx_available());
  });
}

TEST_F(DwApbUartHarness, AsyncReadAndWrite) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  // --- Test Async Read ---
  auto read_future = QueueAsyncRead(client);

  // Inject multi-byte data.
  const uint8_t input_data[] = {10, 20, 30, 40};
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(input_data); });

  // Now, the read should complete.
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), sizeof(input_data));
  ASSERT_EQ(memcmp(read_data.data(), input_data, sizeof(input_data)), 0);

  // --- Test Async Write ---
  std::vector<uint8_t> output_data(20, 0x55);
  auto write_future = QueueAsyncWrite(client, output_data);

  // Verify that the first 16 bytes were written.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 16u);
    ASSERT_EQ(tx_buf[0], 0x55);
  });

  // Clear TX FIFO and trigger TX interrupt.
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject({}); });

  // The write should now complete.
  ASSERT_EQ(ZX_OK, write_future.get());

  // Verify that the remaining 4 bytes were written.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 4u);
    ASSERT_EQ(tx_buf[0], 0x55);
  });
}

TEST_F(DwApbUartHarness, CancelAll) {
  auto client = CreateAsyncClient();

  // Enable
  {
    fdf::Arena arena('ENAB');
    client.buffer(arena)->Enable(true).Then(
        [](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Enable>& result) {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result->is_ok());
        });
  }

  // Queue Read and Write, then CancelAll on the same client.
  std::promise<zx_status_t> read_promise;
  auto read_future = read_promise.get_future();
  std::promise<zx_status_t> write_promise;
  auto write_future = write_promise.get_future();
  std::promise<void> cancel_promise;
  auto cancel_future = cancel_promise.get_future();

  fdf::Arena read_arena('READ');
  fdf::Arena write_arena('WRIT');
  fdf::Arena cancel_arena('CNCL');

  // Queue Read
  client.buffer(read_arena)
      ->Read()
      .Then([p = std::move(read_promise)](
                fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Read>& result) mutable {
        if (!result.ok()) {
          p.set_value(result.status());
        } else if (result->is_error()) {
          p.set_value(result->error_value());
        } else {
          p.set_value(ZX_OK);
        }
      });

  // Queue Write (20 bytes)
  std::vector<uint8_t> output_data(20, 0x77);
  client.buffer(write_arena)
      ->Write(fidl::VectorView<uint8_t>::FromExternal(output_data.data(), output_data.size()))
      .Then(
          [p = std::move(write_promise)](
              fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Write>& result) mutable {
            if (!result.ok()) {
              p.set_value(result.status());
            } else if (result->is_error()) {
              p.set_value(result->error_value());
            } else {
              p.set_value(ZX_OK);
            }
          });

  // Call CancelAll (guaranteed to be processed after Read and Write due to channel order)
  client.buffer(cancel_arena)
      ->CancelAll()
      .Then([p = std::move(cancel_promise)](
                fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::CancelAll>&
                    result) mutable { p.set_value(); });

  // Wait for CancelAll to finish
  cancel_future.wait();

  // Verify both returned CANCELED
  ASSERT_EQ(read_future.get(), ZX_ERR_CANCELED);
  ASSERT_EQ(write_future.get(), ZX_ERR_CANCELED);
}

TEST_F(DwApbUartHarness, WriteConcurrentRequests) {
  auto client = CreateAsyncClient();

  {
    fdf::Arena arena('TEST');
    client.buffer(arena)->Enable(true).Then(
        [](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Enable>& result) {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result->is_ok());
        });
  }

  std::vector<uint8_t> output_data(20, 0xAA);
  std::promise<zx_status_t> write_promise;
  auto write_future = write_promise.get_future();

  {
    fdf::Arena write_arena('WRIT');
    client.buffer(write_arena)
        ->Write(fidl::VectorView<uint8_t>::FromExternal(output_data.data(), output_data.size()))
        .Then([p = std::move(write_promise)](
                  fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Write>&
                      result) mutable {
          if (!result.ok()) {
            p.set_value(result.status());
          } else if (result->is_error()) {
            p.set_value(result->error_value());
          } else {
            p.set_value(ZX_OK);
          }
        });
  }

  // Wait for driver to process first 16 bytes.
  PollUntil([&]() {
    size_t written = 0;
    driver_test().RunInEnvironmentTypeContext(
        [&](Environment& env) { written = env.device_state().TxBufSize(); });
    return written == 16;
  });

  // Verify first 16 bytes.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 16u);
    ASSERT_EQ(tx_buf[0], 0xAA);
  });

  // Interleave a GetInfo call on the same client to verify concurrent requests on the channel
  // do not overwrite or corrupt the in-flight write state.
  std::promise<void> info_promise;
  auto info_future = info_promise.get_future();
  {
    fdf::Arena info_arena('INFO');
    client.buffer(info_arena)
        ->GetInfo()
        .Then([p = std::move(info_promise)](
                  fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::GetInfo>&
                      info_result) mutable {
          ASSERT_TRUE(info_result.ok());
          ASSERT_TRUE(info_result->is_ok());
          p.set_value();
        });
  }
  info_future.wait();

  // Clear TX FIFO and trigger TX interrupt to write the remaining 4 bytes.
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject({}); });

  // Wait for write to complete.
  ASSERT_EQ(write_future.get(), ZX_OK);

  // Verify the remaining 4 bytes.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 4u);
    ASSERT_EQ(tx_buf[0], 0xAA);
  });
}

TEST_F(DwApbUartHarness, MultiInterruptLoop) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  // 1. Start Async Write (20 bytes).
  std::vector<uint8_t> write_data(20, 0xAA);
  auto write_future = AsyncWrite(client, write_data);

  // Wait for driver to process first 16 bytes.
  PollUntil([&]() {
    size_t written = 0;
    driver_test().RunInEnvironmentTypeContext(
        [&](Environment& env) { written = env.device_state().TxBufSize(); });
    return written == 16;
  });

  // Consume the first 16 bytes from mock to clear TX FIFO.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 16u);
    ASSERT_EQ(tx_buf[0], 0xAA);
  });

  // Now TX is empty, TX interrupt is enabled, but not triggered yet.

  // 2. Start Async Read.
  auto read_client = CreateAsyncClient();
  auto read_future = QueueAsyncRead(read_client);

  // 3. Inject RX data. This will trigger the interrupt.
  // Since both RX and TX are pending, the driver must process both in the loop.
  std::vector<uint8_t> rx_data = {1, 2, 3, 4};
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(rx_data); });

  // 4. Wait for both to complete.
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), rx_data.size());
  ASSERT_EQ(memcmp(read_data.data(), rx_data.data(), rx_data.size()), 0);

  ASSERT_EQ(write_future.get(), ZX_OK);

  // 5. Verify the remaining 4 bytes of TX were transmitted.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 4u);
    ASSERT_EQ(tx_buf[0], 0xAA);
  });
}

TEST_F(DwApbUartHarness, ConcurrentReadWriteError) {
  auto client = CreateAsyncClient();

  // Enable
  {
    fdf::Arena arena('ENAB');
    client.buffer(arena)->Enable(true).Then(
        [](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Enable>& result) {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result->is_ok());
        });
  }

  std::promise<zx_status_t> read1_promise;
  auto read1_future = read1_promise.get_future();
  std::promise<zx_status_t> read2_promise;
  auto read2_future = read2_promise.get_future();
  std::promise<zx_status_t> write1_promise;
  auto write1_future = write1_promise.get_future();
  std::promise<zx_status_t> write2_promise;
  auto write2_future = write2_promise.get_future();
  std::promise<void> cancel_promise;
  auto cancel_future = cancel_promise.get_future();

  fdf::Arena read1_arena('REA1');
  fdf::Arena read2_arena('REA2');
  fdf::Arena write1_arena('WRI1');
  fdf::Arena write2_arena('WRI2');
  fdf::Arena cancel_arena('CNCL');

  // 1. Queue Read 1
  client.buffer(read1_arena)
      ->Read()
      .Then([p = std::move(read1_promise)](
                fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Read>& result) mutable {
        if (!result.ok()) {
          p.set_value(result.status());
        } else if (result->is_error()) {
          p.set_value(result->error_value());
        } else {
          p.set_value(ZX_OK);
        }
      });

  // 2. Queue Read 2 (should fail immediately with ALREADY_BOUND)
  client.buffer(read2_arena)
      ->Read()
      .Then([p = std::move(read2_promise)](
                fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Read>& result) mutable {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        p.set_value(result->error_value());
      });

  // 3. Queue Write 1 (20 bytes, will block)
  std::vector<uint8_t> write_data(20, 0x55);
  client.buffer(write1_arena)
      ->Write(fidl::VectorView<uint8_t>::FromExternal(write_data.data(), write_data.size()))
      .Then(
          [p = std::move(write1_promise)](
              fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Write>& result) mutable {
            if (!result.ok()) {
              p.set_value(result.status());
            } else if (result->is_error()) {
              p.set_value(result->error_value());
            } else {
              p.set_value(ZX_OK);
            }
          });

  // 4. Queue Write 2 (should fail immediately with ALREADY_BOUND)
  std::vector<uint8_t> write_data2(5, 0x66);
  client.buffer(write2_arena)
      ->Write(fidl::VectorView<uint8_t>::FromExternal(write_data2.data(), write_data2.size()))
      .Then(
          [p = std::move(write2_promise)](
              fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Write>& result) mutable {
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_error());
            p.set_value(result->error_value());
          });

  // 5. Clean up pending operations
  client.buffer(cancel_arena)
      ->CancelAll()
      .Then([p = std::move(cancel_promise)](
                fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::CancelAll>&
                    result) mutable { p.set_value(); });

  // Verify concurrent errors returned immediately
  ASSERT_EQ(read2_future.get(), ZX_ERR_ALREADY_BOUND);
  ASSERT_EQ(write2_future.get(), ZX_ERR_ALREADY_BOUND);

  // Wait for CancelAll to finish
  cancel_future.wait();

  // Verify pending operations were cancelled
  ASSERT_EQ(read1_future.get(), ZX_ERR_CANCELED);
  ASSERT_EQ(write1_future.get(), ZX_ERR_CANCELED);
}

TEST_F(DwApbUartHarness, RxLineStatusErrorInterrupt) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  auto read_future = QueueAsyncRead(client);

  // Inject some data into the mock RX buffer and set line status errors (OE, PE, FE, BI).
  const uint8_t input_data[] = {0x11, 0x22, 0x33};
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    // Set Line Status Error: Overrun(bit 1), Parity(bit 2), Framing(bit 3), Break(bit 4)
    env.device_state().Inject(input_data, 0x1E);
  });

  // Verify that the driver recovered, LSR was cleared, and data was read.
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), sizeof(input_data));
  ASSERT_EQ(memcmp(read_data.data(), input_data, sizeof(input_data)), 0);
}

TEST_F(DwApbUartHarness, CharacterTimeoutInterrupt) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  auto read_future = QueueAsyncRead(client);

  // Inject partial data and trigger a Character Timeout interrupt (IIR = 0x0C).
  const uint8_t input_data[] = {0xDE, 0xAD, 0xBE, 0xEF};
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.device_state().Inject(input_data);
    env.device_state().SetCharTimeout(true);
  });

  // Verify the driver drains the RX FIFO on character timeout into its buffer.
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), sizeof(input_data));
  ASSERT_EQ(memcmp(read_data.data(), input_data, sizeof(input_data)), 0);
}

TEST_F(DwApbUartHarness, BusyDetectInterrupt) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  // Trigger Busy Detect interrupt (0x07).
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().SetBusyDetect(true); });

  // Poll until the driver services the Busy Detect interrupt and reads USR to clear it.
  PollUntil([&]() {
    bool busy_pending = true;
    driver_test().RunInEnvironmentTypeContext(
        [&](Environment& env) { busy_pending = env.device_state().busy_detect_irq_pending(); });
    return !busy_pending;
  });

  auto read_future = QueueAsyncRead(client);

  // Verify driver remains fully functional by injecting data.
  const uint8_t input_data[] = {0x42};
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(input_data); });

  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), 1u);
  ASSERT_EQ(read_data[0], 0x42);
}

TEST_F(DwApbUartHarness, ModemStatusInterrupt) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  // Set delta CTS (bit 0) in MSR and trigger modem status interrupt.
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().SetModemStatus(0x01); });

  // Poll until driver services Modem Status interrupt and reads MSR to clear it.
  PollUntil([&]() {
    bool modem_pending = true;
    driver_test().RunInEnvironmentTypeContext(
        [&](Environment& env) { modem_pending = env.device_state().modem_status_irq_pending(); });
    return !modem_pending;
  });

  auto read_future = QueueAsyncRead(client);

  // Verify driver remains fully functional by injecting data.
  const uint8_t input_data[] = {0x99};
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(input_data); });

  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), 1u);
  ASSERT_EQ(read_data[0], 0x99);
}

TEST_F(DwApbUartHarness, MultiInterruptPriorityPreemption) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  // Start an async write of 20 bytes.
  std::vector<uint8_t> write_data(20, 0x33);
  auto write_future = AsyncWrite(client, write_data);

  // Wait for the driver to fill the initial 16 bytes into the TX buffer.
  PollUntil([&]() {
    size_t written = 0;
    driver_test().RunInEnvironmentTypeContext(
        [&](Environment& env) { written = env.device_state().TxBufSize(); });
    return written == 16;
  });

  // Consume first 16 bytes from mock to empty TX FIFO.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 16u);
  });

  // Simultaneously queue a Read and assert Line Status Error, RX data, and TX Empty.
  auto read_client = CreateAsyncClient();
  auto read_future = QueueAsyncRead(read_client);

  // Inject RX data and Line Status Error.
  std::vector<uint8_t> rx_data = {0xA1, 0xB2, 0xC3};
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.device_state().Inject(rx_data, 0x02);  // OE
  });

  // Verify Read completes and Line Status error was cleared.
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), rx_data.size());
  ASSERT_EQ(memcmp(read_data.data(), rx_data.data(), rx_data.size()), 0);

  // Verify Write also completes (serviced after RX in priority order).
  ASSERT_EQ(write_future.get(), ZX_OK);

  // Verify remaining 4 bytes of TX were transmitted.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), 4u);
    ASSERT_EQ(tx_buf[0], 0x33);
    ASSERT_EQ(env.device_state().line_status_error(), 0u);
  });
}

TEST_F(DwApbUartHarness, BaudRateConfigBusyPolling) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  // Set mock to report busy for 5 USR reads, simulating transient hardware busy state.
  driver_test().RunInEnvironmentTypeContext(
      [](Environment& env) { env.device_state().SetBusyCycles(5); });

  static constexpr uint32_t serial_test_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  // Verify divisors were configured properly after busy cleared.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    ASSERT_EQ(env.device_state().dll(), 109u);
    ASSERT_EQ(env.device_state().dlh(), 0u);
  });
}

TEST_F(DwApbUartHarness, BaudRateConfigBusyTimeout) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  // Set mock to report permanently busy.
  driver_test().RunInEnvironmentTypeContext(
      [](Environment& env) { env.device_state().SetAlwaysBusy(true); });

  static constexpr uint32_t serial_test_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  // The call should fail with ZX_ERR_TIMED_OUT because USR[0] never clears.
  ASSERT_TRUE(result->is_error());
  ASSERT_EQ(result->error_value(), ZX_ERR_TIMED_OUT);

  // Restore mock.
  driver_test().RunInEnvironmentTypeContext(
      [](Environment& env) { env.device_state().SetAlwaysBusy(false); });
}

TEST_F(DwApbUartHarness, DlfProgrammingOrder) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  // Enable DLF support in mock CPR.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    env.device_state().set_additional_feat(true);
    env.device_state().ClearWriteLog();
  });

  static constexpr uint32_t serial_test_config =
      fuchsia_hardware_serialimpl::kSerialSetBaudRateOnly;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  // Verify write order: DLF (0xc0) MUST be written before DLL (0x00) and DLH (0x04)
  // per Synopsys APB UART Databook Section 6.4.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    const auto& log = env.device_state().write_log();
    int dlf_idx = -1;
    int dll_idx = -1;
    int dlh_idx = -1;

    for (size_t i = 0; i < log.size(); ++i) {
      if (log[i].offset == 0xc0 && dlf_idx == -1) {
        dlf_idx = static_cast<int>(i);
      } else if (log[i].offset == 0x00 && dll_idx == -1) {
        dll_idx = static_cast<int>(i);
      } else if (log[i].offset == 0x04 && dlh_idx == -1) {
        dlh_idx = static_cast<int>(i);
      }
    }

    ASSERT_NE(dlf_idx, -1);
    ASSERT_NE(dll_idx, -1);
    ASSERT_NE(dlh_idx, -1);
    ASSERT_LT(dlf_idx, dll_idx);
    ASSERT_LT(dlf_idx, dlh_idx);
  });
}

TEST_F(DwApbUartHarness, LegacyHardwareFallback) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  // Simulate legacy hardware where CPR reports uart_add_encoded_params = false.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    env.device_state().set_uart_add_encoded_params(false);
    env.device_state().set_additional_feat(true);  // Should be ignored because CPR is invalid
    env.device_state().ClearWriteLog();
  });

  static constexpr uint32_t serial_test_config = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                                 fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                                 fuchsia_hardware_serialimpl::kSerialParityNone;
  auto result = client.buffer(arena)->Config(115200, serial_test_config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());

  // Verify that integer divisor is used (109) and DLF is NOT written.
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    ASSERT_EQ(env.device_state().dll(), 109u);
    ASSERT_EQ(env.device_state().dlh(), 0u);

    // Verify DLF (0xc0) was never written in the write log.
    for (const auto& entry : env.device_state().write_log()) {
      ASSERT_NE(entry.offset, 0xc0u);
    }
  });
}

TEST_F(DwApbUartHarness, AllConcurrentInterruptPrioritiesFullCascade) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  // Set up Async Write (10 bytes) and barrier to ensure it is queued in driver
  std::vector<uint8_t> write_data = {0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0};
  auto write_future = QueueAsyncWrite(client, write_data);

  // Set up Async Read on the async client
  auto read_future = QueueAsyncRead(client);

  // Simultaneously assert ALL 5 interrupt conditions in the hardware mock:
  // 1. Line Status Error (OE, PE, FE, BI)
  // 2. RX Data Available
  // 3. TX Empty (ready to accept data)
  // 4. Modem Status (delta bits)
  // 5. Busy Detect
  std::vector<uint8_t> rx_data = {0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE};
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.device_state().AssertAllInterruptConditions(rx_data, 0x1E, 0x0F, true);
  });

  // Wait for Read and Write to complete
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), rx_data.size());
  ASSERT_EQ(memcmp(read_data.data(), rx_data.data(), rx_data.size()), 0);

  ASSERT_EQ(write_future.get(), ZX_OK);

  // Poll until all interrupt flags are cleared by the driver in priority order
  PollUntil([&]() {
    bool all_cleared = false;
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      all_cleared = (env.device_state().line_status_error() == 0) &&
                    (!env.device_state().modem_status_irq_pending()) &&
                    (!env.device_state().busy_detect_irq_pending());
    });
    return all_cleared;
  });

  // Verify full state cleanliness on the mock
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    ASSERT_EQ(env.device_state().line_status_error(), 0u);
    ASSERT_FALSE(env.device_state().modem_status_irq_pending());
    ASSERT_FALSE(env.device_state().busy_detect_irq_pending());

    auto tx_buf = env.device_state().TxBuf();
    ASSERT_EQ(tx_buf.size(), write_data.size());
    ASSERT_EQ(memcmp(tx_buf.data(), write_data.data(), write_data.size()), 0);
  });
}

TEST_F(DwApbUartHarness, RingBufferOverflowGatingAndBackpressure) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  {
    auto result = client.buffer(arena)->Enable(true);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  // 1. Generate exactly kRxBufferSize (16,384 bytes) of deterministic pattern data
  std::vector<uint8_t> fill_data(serial::DwApbUart::kRxBufferSize);
  for (size_t i = 0; i < fill_data.size(); ++i) {
    fill_data[i] = static_cast<uint8_t>(i & 0xFF);
  }

  // Inject fill data into the mock RX buffer
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(fill_data); });

  // Poll until the driver fills its ring buffer and disables the RX interrupt (gating)
  PollUntil([&]() {
    bool rx_disabled = false;
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
      rx_disabled = !ier.rx_available();
    });
    return rx_disabled;
  });

  // Verify RX interrupt is currently disabled
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
    ASSERT_FALSE(ier.rx_available());
  });

  // 2. Read the entire 16,384 bytes from the driver
  {
    fdf::Arena read_arena('READ');
    auto result = client.buffer(read_arena)->Read();
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    ASSERT_EQ(result->value()->data.size(), serial::DwApbUart::kRxBufferSize);
    ASSERT_EQ(memcmp(result->value()->data.data(), fill_data.data(), fill_data.size()), 0);
  }

  // 3. Verify that the RX interrupt was re-enabled automatically upon buffer drain
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    auto ier = uart::dw8250::InterruptEnableRegister::Get().FromValue(env.device_state().ier());
    ASSERT_TRUE(ier.rx_available());
  });

  // Sync barrier: ensure the driver dispatcher has processed to idle and virtual interrupt
  // is acknowledged before injecting next data.
  {
    fdf::Arena barrier_arena('BARR');
    auto info_res = client.buffer(barrier_arena)->GetInfo();
    ASSERT_TRUE(info_res.ok());
    ASSERT_TRUE(info_res->is_ok());
  }

  // 4. Inject a subsequent batch of 256 bytes to verify continuous operational readiness
  std::vector<uint8_t> next_batch(256, 0x7E);
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(next_batch); });

  {
    fdf::Arena read_arena2('REA2');
    auto result2 = client.buffer(read_arena2)->Read();
    ASSERT_TRUE(result2.ok());
    ASSERT_TRUE(result2->is_ok());
    ASSERT_EQ(result2->value()->data.size(), next_batch.size());
    ASSERT_EQ(memcmp(result2->value()->data.data(), next_batch.data(), next_batch.size()), 0);
  }
}

TEST_F(DwApbUartHarness, RingBufferLargeScaleFuzzing) {
  auto client = CreateClient();
  fdf::Arena arena('TEST');

  {
    auto result = client.buffer(arena)->Enable(true);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }

  // Fuzz 64KB of streaming data through varied chunk sizes (512, 1024, 2048, 4096, 8192)
  constexpr size_t kTotalBytes = 65536;
  std::vector<uint8_t> master_stream(kTotalBytes);
  for (size_t i = 0; i < kTotalBytes; ++i) {
    master_stream[i] = static_cast<uint8_t>((i * 13 + 7) & 0xFF);
  }

  const size_t chunk_sizes[] = {512, 1024, 2048, 4096, 8192, 4096, 2048, 1024, 512, 41984};
  size_t offset = 0;

  for (size_t chunk_size : chunk_sizes) {
    if (offset >= kTotalBytes) {
      break;
    }
    size_t cur_chunk = std::min(chunk_size, kTotalBytes - offset);

    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      env.device_state().Inject(std::span{master_stream}.subspan(offset, cur_chunk));
    });

    size_t chunk_received = 0;
    while (chunk_received < cur_chunk) {
      fdf::Arena read_arena('FUZZ');
      auto result = client.buffer(read_arena)->Read();
      ASSERT_TRUE(result.ok());
      ASSERT_TRUE(result->is_ok());
      size_t n = result->value()->data.size();
      ASSERT_GT(n, 0u);
      ASSERT_EQ(
          memcmp(result->value()->data.data(), master_stream.data() + offset + chunk_received, n),
          0);
      chunk_received += n;
    }
    ASSERT_EQ(chunk_received, cur_chunk);

    // Sync barrier: ensure the driver's HandleIrq has completely returned and acknowledged
    // the virtual interrupt before we inject the next chunk. Calling zx_interrupt_trigger()
    // on a virtual interrupt while the driver is still servicing the previous interrupt
    // (before irq_.ack()) causes Zircon to silently drop the trigger.
    {
      fdf::Arena barrier_arena('BARR');
      auto info_res = client.buffer(barrier_arena)->GetInfo();
      ASSERT_TRUE(info_res.ok());
      ASSERT_TRUE(info_res->is_ok());
    }

    offset += cur_chunk;
  }

  ASSERT_EQ(offset, kTotalBytes);
}

TEST_F(DwApbUartHarness, RxFifoErrorOnlyInLsr) {
  auto client = CreateAsyncClient();
  AsyncEnable(client);

  auto read_future = QueueAsyncRead(client);

  // Set bit 7 (error_in_rx_fifo = 0x80) without bits 1-4 atomically with Inject
  const uint8_t input_data[] = {0x55, 0x66, 0x77};
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.device_state().Inject(input_data, 0x80); });

  // Read will complete as soon as driver handles the line status error interrupt and drains the
  // FIFO.
  auto read_data = read_future.get();
  ASSERT_EQ(read_data.size(), sizeof(input_data));
  ASSERT_EQ(memcmp(read_data.data(), input_data, sizeof(input_data)), 0);
}

class AlreadyBoundPDevServer final
    : public fidl::testing::WireTestBase<fuchsia_hardware_platform_device::Device> {
 public:
  fuchsia_hardware_platform_device::Service::InstanceHandler GetInstanceHandler(
      async_dispatcher_t* dispatcher) {
    return fuchsia_hardware_platform_device::Service::InstanceHandler({
        .device = binding_group_.CreateHandler(
            this, dispatcher ? dispatcher : async_get_default_dispatcher(),
            fidl::kIgnoreBindingClosure),
    });
  }

  void GetInterruptById(GetInterruptByIdRequestView request,
                        GetInterruptByIdCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_platform_device::Device> binding_group_;
};

class AlreadyBoundEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    constexpr std::string_view kInstanceName = "pdev";
    zx::result add_service_result =
        to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
            pdev_server_.GetInstanceHandler(dispatcher), kInstanceName);
    ZX_ASSERT(add_service_result.is_ok());

    fake_clock_.set_rate(200000000);

    auto add_clock_apb = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
        fake_clock_.CreateInstanceHandler(dispatcher), "apb_pclk");
    ZX_ASSERT(add_clock_apb.is_ok());
    auto add_clock_baud = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
        fake_clock_.CreateInstanceHandler(dispatcher), "baudclk");
    ZX_ASSERT(add_clock_baud.is_ok());

    auto add_reset = to_driver_vfs.AddService<fuchsia_hardware_reset::Service>(
        fake_reset_.CreateInstanceHandler(), "reset");
    ZX_ASSERT(add_reset.is_ok());

    auto add_power = to_driver_vfs.AddService<fuchsia_hardware_powerdomain::Service>(
        fake_power_domain_.CreateInstanceHandler(), "power-domain");
    ZX_ASSERT(add_power.is_ok());

    return zx::ok();
  }

  fdf_fake::FakeClock& fake_clock() { return fake_clock_; }
  fdf_fake::FakeReset& fake_reset() { return fake_reset_; }
  fdf_fake::FakePowerDomain& fake_power_domain() { return fake_power_domain_; }

 private:
  AlreadyBoundPDevServer pdev_server_;
  fdf_fake::FakeClock fake_clock_;
  fdf_fake::FakeReset fake_reset_;
  fdf_fake::FakePowerDomain fake_power_domain_;
};

class AlreadyBoundConfig {
 public:
  using DriverType = serial::DwApbUartDriver;
  using EnvironmentType = AlreadyBoundEnvironment;
};

TEST(DwApbUartStartTest, StartFailsGracefullyWhenInterruptAlreadyBound) {
  fdf_testing::BackgroundDriverTest<AlreadyBoundConfig> driver_test;
  zx::result result = driver_test.StartDriver();
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.status_value(), ZX_ERR_ALREADY_BOUND);

  // Verify that clocks, power domain, and reset lines were completely untouched.
  driver_test.RunInEnvironmentTypeContext([](AlreadyBoundEnvironment& env) {
    EXPECT_FALSE(env.fake_power_domain().is_enabled());
    EXPECT_FALSE(env.fake_clock().enabled());
    EXPECT_FALSE(env.fake_reset().take_asserted());
    EXPECT_FALSE(env.fake_reset().take_deasserted());
  });
}

}  // namespace
