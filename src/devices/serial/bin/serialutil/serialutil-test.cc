// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "serialutil.h"

#include <fidl/fuchsia.hardware.serial/cpp/wire.h>
#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/wire.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fit/function.h>
#include <zircon/types.h>

#include <cstdint>

#include <gtest/gtest.h>
#include <sdk/lib/driver/testing/cpp/driver_runtime.h>
#include <src/lib/testing/predicates/status.h>

#include "serial.h"
#include "src/devices/serial/drivers/serial/serial_config.h"

namespace {

// Fake for the SerialImpl protocol. The real serial driver will get wired up to this. All
// operations except for `Bind` and `Read` are no-ops.
class FakeSerialImpl : public fdf::WireServer<fuchsia_hardware_serialimpl::Device> {
 public:
  FakeSerialImpl() = default;

  // The number of times `Read` has been called.
  unsigned reads() const { return reads_; }

  void Bind(fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server) {
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->get(), std::move(server), this,
                         fidl::kIgnoreBindingClosure);
  }

  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess(
        {.serial_class = fuchsia_hardware_serial::Class::kGeneric});
  }

  void Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }

  void Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }

  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override {
    // Track the number of read calls.
    reads_++;

    // Just respond with a canned response.
    uint8_t reply[] = {'h', 'i', 0};
    fidl::VectorView<uint8_t> buffer(arena, reply);
    completer.buffer(arena).ReplySuccess(buffer);
  }

  void Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request, fdf::Arena& arena,
             WriteCompleter::Sync& completer) override {
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
  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> bindings_;
  unsigned reads_ = 0;
};

struct SerialTestEnvironment : public fdf_testing::Environment {
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    device_server_.Initialize(component::kDefaultInstance);

    zx_status_t status =
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs);
    if (status != ZX_OK) {
      return zx::error(status);
    }

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
  compat::DeviceServer device_server_;
};

// Config used for the `fdf_testing::ForegroundDriverTest` class.
class FixtureConfig final {
 public:
  using DriverType = serial::SerialDevice;
  using EnvironmentType = SerialTestEnvironment;
};

// Test fixture for testing the `serialutil` program. Provides helpers for "launching" the
// program and accessing the backing driver.
class SerialTest : public ::testing::Test {
 public:
  void SetUp() override {
    EXPECT_TRUE(driver_test_
                    .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                      serial_config::Config config{{.enable_suspend = true}};
                      args.config(config.ToVmo());
                    })
                    .is_ok());
  }
  void TearDown() override { EXPECT_OK(driver_test_.StopDriver()); }

  // Provides access to the `FakeSerialImpl` driver instance that has been loaded in the test
  // environment.
  //
  // @param callable A function to run in the context of the test environment that will be provided
  // access to the FakeSerialImpl instance.
  // @return The return value from the `callable`.
  template <typename Callable>
  auto serial_impl(Callable&& callable) {
    return driver_test_
        .RunInEnvironmentTypeContext<std::invoke_result_t<Callable, FakeSerialImpl&>>(
            [callback = std::forward<Callable>(callable)](SerialTestEnvironment& env) mutable {
              return callback(env.serial_impl);
            });
  }

  // Checks that number of read operations on the driver is the expected value.
  //
  // @param expected The expected number of reads.
  void AssertReadCountIs(unsigned expected) {
    serial_impl(
        [expected](FakeSerialImpl& serial_impl) { ASSERT_EQ(serial_impl.reads(), expected); });
  }

  // Runs the `serialutil` command and checks that it returns the expected value.
  //
  // @param args The command line.
  // @param expected The expected return value for the `serialutil` program.
  template <size_t kSize>
  void CallSerialUtil(std::span<std::string, kSize> args, int expected) {
    // Setup an argc and argv for the serialutil main function.
    int argc = static_cast<int>(kSize);
    std::array<char*, kSize> argv{};
    for (unsigned i = 0; i < kSize; i++) {
      argv[i] = args[i].data();
    }

    // `serialutil` uses synchronous calls so we need to run it in the background dispatcher to
    // allow the driver to respond to the FIDL requests.
    auto run = [this, expected, argc, argv = argv.data()]() {
      // Connect to "devfs". During testing the `/dev/class` directory isn't available, instead we
      // use the test framework's `ConnectThroughDevfs` which connects to the nodes directly instead
      // of through a filesystem proxy. We pass the pre-connected devfs connection to `serialutil`
      // to us during testing, otherwise it would attempt to open `/dev/class/serial` and fail.
      constexpr std::string_view kDevfsPath{"serial"};
      zx::result controller =
          driver_test_.ConnectThroughDevfs<fuchsia_hardware_serial::DeviceProxy>(kDevfsPath);
      EXPECT_OK(controller);

      // Act like a standard program invocation of `serialutil`.
      EXPECT_EQ(expected, serial::SerialUtil::Execute(argc, argv, std::move(*controller)));
    };

    EXPECT_OK(driver_test_.RunOnBackgroundDispatcherSync(std::move(run)));
  }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

// Tests a default invocation:
//  `serialutil`
TEST_F(SerialTest, Default) {
  std::string args[]{"serialutil"};
  CallSerialUtil(std::span(args), 0);
  AssertReadCountIs(1);
}

// Tests an invocation that specifies a device:
//  `serialutil --dev /dev/class/serial/fake_device`
TEST_F(SerialTest, SpecificDevice) {
  std::string args[]{"serialutil", "--dev", "/dev/class/serial/fake_device"};
  CallSerialUtil(std::span(args), 0);
  AssertReadCountIs(1);
}

// Tests an invocation that specifies a number of iterations to read:
//  `serialutil --iter 5`
TEST_F(SerialTest, Iterations) {
  std::string args[]{"serialutil", "--iter", "5"};
  CallSerialUtil(std::span(args), 0);
  AssertReadCountIs(5);
}

// Tests an invocation that specifies a number of iterations to read:
//  `serialutil --iter 7 --dev /dev/class/serial/fake_device`
TEST_F(SerialTest, SpecificDeviceAndIterations) {
  std::string args[]{"serialutil", "--iter", "7", "--dev", "/dev/class/serial/fake_device"};
  CallSerialUtil(std::span(args), 0);
  AssertReadCountIs(7);
}

// Tests an invocation that specifies an invalid param:
//  `serialutil ohno`
TEST_F(SerialTest, BadArg) {
  std::string args[]{"serialutil", "ohno"};
  CallSerialUtil(std::span(args), -1);
  AssertReadCountIs(0);
}

// Tests an invocation that requests the program usage:
//  `serialutil --help`
TEST_F(SerialTest, Help) {
  std::string args[]{"serialutil", "--help"};
  CallSerialUtil(std::span(args), 0);
  AssertReadCountIs(0);
}
}  // namespace
