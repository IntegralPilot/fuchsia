// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "serial.h"

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fit/function.h>

#include <gtest/gtest.h>
#include <sdk/lib/driver/testing/cpp/driver_runtime.h>
#include <src/lib/testing/predicates/status.h>

#include "src/devices/serial/drivers/serial/serial_config.h"

namespace {

constexpr size_t kBufferLength = 16;

constexpr zx_signals_t kEventWrittenSignal = ZX_USER_SIGNAL_0;

// Fake for the SerialImpl protocol.
class FakeSerialImpl : public fdf::WireServer<fuchsia_hardware_serialimpl::Device> {
 public:
  FakeSerialImpl() { zx::event::create(0, &write_event_); }

  // Getters.
  bool enabled() const { return enabled_; }
  void set_enabled(bool enabled) { enabled_ = enabled; }

  uint8_t* read_buffer() { return read_buffer_; }
  const uint8_t* write_buffer() { return write_buffer_; }
  size_t write_buffer_length() const { return write_buffer_length_; }

  size_t total_written_bytes() const { return total_written_bytes_; }

  // Test utility methods.
  zx_status_t wait_for_write(zx::time deadline, zx_signals_t* pending) {
    return write_event_.wait_one(kEventWrittenSignal, deadline, pending);
  }

  void Bind(fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server) {
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->get(), std::move(server), this,
                         fidl::kIgnoreBindingClosure);
  }

  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess({fuchsia_hardware_serial::Class::kGeneric});
  }

  void Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }

  void Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override {
    enabled_ = request->enable;
    completer.buffer(arena).ReplySuccess();
  }

  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override {
    fidl::VectorView<uint8_t> buffer(arena, kBufferLength);
    size_t i;

    for (i = 0; i < kBufferLength && read_buffer_[i]; ++i) {
      buffer[i] = read_buffer_[i];
    }
    buffer.set_size(i);

    completer.buffer(arena).ReplySuccess(buffer);
  }

  void Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request, fdf::Arena& arena,
             WriteCompleter::Sync& completer) override {
    size_t i;

    for (i = 0; i < request->data.size() && i < kBufferLength; ++i) {
      write_buffer_[i] = request->data[i];
    }

    // Signal that the write_buffer has been written to.
    if (i > 0) {
      write_buffer_length_ = i;
      total_written_bytes_ += i;
      write_event_.signal(0, kEventWrittenSignal);
    }

    completer.buffer(arena).ReplySuccess();
  }

  void CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) override {
    completer.buffer(arena).Reply();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

 private:
  bool enabled_;

  uint8_t read_buffer_[kBufferLength];
  uint8_t write_buffer_[kBufferLength];
  size_t write_buffer_length_;
  size_t total_written_bytes_ = 0;

  zx::event write_event_;

  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> bindings_;
};

struct SerialTestEnvironment : public fdf_testing::Environment {
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    fuchsia_hardware_serialimpl::Service::InstanceHandler instance_handler({
        .device =
            [this](fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server) {
              serial_impl.Bind(std::move(server));
            },
    });
    return to_driver_vfs.AddService<fuchsia_hardware_serialimpl::Service>(
        std::move(instance_handler));
  }

  FakeSerialImpl serial_impl;
};

class FixtureConfig final {
 public:
  using DriverType = serial::SerialDevice;
  using EnvironmentType = SerialTestEnvironment;
};

class SerialTest : public ::testing::Test {
 public:
  template <typename Callable>
  auto serial_impl(Callable&& callable) {
    return driver_test()
        .RunInEnvironmentTypeContext<std::invoke_result_t<Callable, FakeSerialImpl&>>(
            [callback = std::forward<Callable>(callable)](SerialTestEnvironment& env) mutable {
              return callback(env.serial_impl);
            });
  }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(SerialTest, Lifetime) {
  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    serial_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    EXPECT_EQ(node.children().count("serial"), 1ul);
    EXPECT_EQ(node.children().count("serial-impl"), 1ul);
  });

  zx::result client_end = driver_test().Connect<fuchsia_hardware_serial::Service::Device>();
  EXPECT_TRUE(client_end.is_ok());

  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(serial_impl([](FakeSerialImpl& serial_impl) { return serial_impl.enabled(); }));

  EXPECT_TRUE(driver_test().StopDriver().is_ok());

  EXPECT_FALSE(serial_impl([](FakeSerialImpl& serial_impl) { return serial_impl.enabled(); }));
}

TEST_F(SerialTest, Read) {
  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    serial_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  zx::result client_end = driver_test().Connect<fuchsia_hardware_serial::Service::Device>();
  EXPECT_TRUE(client_end.is_ok());
  fidl::WireClient client(*std::move(client_end),
                          fdf::Dispatcher::GetCurrent()->async_dispatcher());

  constexpr std::string_view data = "test";

  // Test set up.
  serial_impl([&data](FakeSerialImpl& serial_impl) {
    *std::copy(data.begin(), data.end(), serial_impl.read_buffer()) = 0;
  });

  // Test.
  client->Read().ThenExactlyOnce(
      [this, want = data](fidl::WireUnownedResult<fuchsia_hardware_serial::Device::Read>& result) {
        ASSERT_OK(result.status());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok()) << zx_status_get_string(response.error_value());
        const cpp20::span data = response.value()->data.get();
        const std::string_view got{reinterpret_cast<const char*>(data.data()), data.size_bytes()};
        ASSERT_EQ(got, want);
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(SerialTest, Write) {
  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    serial_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());
  zx::result client_end = driver_test().Connect<fuchsia_hardware_serial::Service::Device>();
  EXPECT_TRUE(client_end.is_ok());
  fidl::WireClient client(*std::move(client_end),
                          fdf::Dispatcher::GetCurrent()->async_dispatcher());

  constexpr std::string_view data = "test";
  uint8_t payload[data.size()];
  std::copy(data.begin(), data.end(), payload);

  // Test.
  client->Write(fidl::VectorView<uint8_t>::FromExternal(payload))
      .ThenExactlyOnce(
          [this,
           want = data](fidl::WireUnownedResult<fuchsia_hardware_serial::Device::Write>& result) {
            ASSERT_OK(result.status());
            const fit::result response = result.value();
            ASSERT_TRUE(response.is_ok()) << zx_status_get_string(response.error_value());
            serial_impl([&want](FakeSerialImpl& serial_impl) {
              const std::string_view got{reinterpret_cast<const char*>(serial_impl.write_buffer()),
                                         serial_impl.write_buffer_length()};
              ASSERT_EQ(got, want);
            });
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

}  // namespace
