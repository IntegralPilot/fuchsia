// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gpioutil.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/mock-function/mock-function.h>
#include <lib/zx/clock.h>
#include <zircon/types.h>

#include <zxtest/zxtest.h>

namespace {

using fuchsia_hardware_gpio::Gpio;

class FakeGpio : public fidl::WireServer<Gpio>,
                 public fidl::WireServer<fuchsia_hardware_pin::Pin>,
                 public fidl::WireServer<fuchsia_hardware_pin::Debug> {
 public:
  explicit FakeGpio(async_dispatcher_t* dispatcher, uint32_t pin = 0,
                    std::string_view name = "NO_NAME")
      : dispatcher_(dispatcher), pin_(pin), name_(name) {}

  void GetProperties(GetPropertiesCompleter::Sync& completer) override {
    mock_get_pin_.Call();
    mock_get_name_.Call();
    fidl::Arena arena;
    auto properties = fuchsia_hardware_pin::wire::DebugGetPropertiesResponse::Builder(arena)
                          .pin(pin_)
                          .name(fidl::StringView::FromExternal(name_))
                          .Build();
    completer.Reply(properties);
  }

  void ConnectPin(fuchsia_hardware_pin::wire::DebugConnectPinRequest* request,
                  ConnectPinCompleter::Sync& completer) override {
    fidl::BindServer(dispatcher_, std::move(request->server), this);
    completer.ReplySuccess();
  }

  void ConnectGpio(fuchsia_hardware_pin::wire::DebugConnectGpioRequest* request,
                   ConnectGpioCompleter::Sync& completer) override {
    fidl::BindServer(dispatcher_, std::move(request->server), this);
    completer.ReplySuccess();
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Debug> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL("Unknown method ordinal 0x%016lx", metadata.method_ordinal);
  }

  void Configure(fuchsia_hardware_pin::wire::PinConfigureRequest* request,
                 ConfigureCompleter::Sync& completer) override {
    if (request->config.has_pull()) {
      mock_set_pull_.Call(request->config.pull());
    }
    if (request->config.has_drive_strength_ua()) {
      mock_set_drive_strength_.Call(request->config.drive_strength_ua());
    }
    if (request->config.has_function()) {
      mock_set_function_.Call(request->config.function());
    }
    if (request->config.has_function_name()) {
      mock_set_function_name_.Call(std::string(request->config.function_name().get()));
    }

    if (!request->config.has_pull() && !request->config.has_drive_strength_ua() &&
        !request->config.has_function() && !request->config.has_function_name()) {
      fidl::Arena arena;
      completer.ReplySuccess(fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                                 .drive_strength_ua(mock_get_drive_strength_.Call())
                                 .Build());
    } else {
      completer.ReplySuccess(request->config);
    }
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Pin> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL("Unknown method ordinal 0x%016lx", metadata.method_ordinal);
  }

  void Read(ReadCompleter::Sync& completer) override { completer.ReplySuccess(mock_read_.Call()); }
  void SetBufferMode(SetBufferModeRequestView request,
                     SetBufferModeCompleter::Sync& completer) override {
    mock_set_buffer_mode_.Call(request->mode);
    completer.ReplySuccess();
  }
  void GetInterrupt(GetInterruptRequestView request,
                    GetInterruptCompleter::Sync& completer) override {
    if (client_got_interrupt_) {
      return completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    }

    zx::interrupt interrupt;
    zx_status_t status = zx::interrupt::create(zx::resource{}, 0, ZX_INTERRUPT_VIRTUAL, &interrupt);
    if (status != ZX_OK) {
      return completer.ReplyError(status);
    }

    // Trigger the interrupt before returning it so that the client's call to zx_interrupt_wait
    // completes immediately.
    if ((status = interrupt.trigger(0, zx::clock::get_boot())); status != ZX_OK) {
      return completer.ReplyError(status);
    }

    client_got_interrupt_ = true;
    mock_get_interrupt_.Call();
    completer.ReplySuccess(std::move(interrupt));
  }
  void ConfigureInterrupt(fuchsia_hardware_gpio::wire::GpioConfigureInterruptRequest* request,
                          ConfigureInterruptCompleter::Sync& completer) override {
    ASSERT_TRUE(request->config.has_mode());
    mock_configure_interrupt_.Call(request->config.mode());
    completer.ReplySuccess();
  }
  void ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) override {
    if (!client_got_interrupt_) {
      return completer.ReplyError(ZX_ERR_NOT_FOUND);
    }

    mock_release_interrupt_.Call();
    completer.ReplySuccess();
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL("Unknown method ordinal 0x%016lx", metadata.method_ordinal);
  }

  mock_function::MockFunction<void>& MockGetPin() { return mock_get_pin_; }
  mock_function::MockFunction<void>& MockGetName() { return mock_get_name_; }
  mock_function::MockFunction<bool>& MockRead() { return mock_read_; }
  mock_function::MockFunction<void, fuchsia_hardware_gpio::BufferMode>& MockSetBufferMode() {
    return mock_set_buffer_mode_;
  }
  mock_function::MockFunction<void, uint64_t>& MockSetDriveStrength() {
    return mock_set_drive_strength_;
  }
  mock_function::MockFunction<uint64_t>& MockGetDriveStrength() { return mock_get_drive_strength_; }
  mock_function::MockFunction<void>& MockGetInterrupt() { return mock_get_interrupt_; }
  mock_function::MockFunction<void, fuchsia_hardware_gpio::InterruptMode>&
  MockConfigureInterrupt() {
    return mock_configure_interrupt_;
  }
  mock_function::MockFunction<void>& MockReleaseInterrupt() { return mock_release_interrupt_; }
  mock_function::MockFunction<void, uint64_t>& MockSetFunction() { return mock_set_function_; }
  mock_function::MockFunction<void, std::string>& MockSetFunctionName() {
    return mock_set_function_name_;
  }
  mock_function::MockFunction<void, fuchsia_hardware_pin::Pull>& MockSetPull() {
    return mock_set_pull_;
  }

 private:
  async_dispatcher_t* const dispatcher_;
  const uint32_t pin_;
  const std::string_view name_;
  mock_function::MockFunction<void> mock_get_pin_;
  mock_function::MockFunction<void> mock_get_name_;
  mock_function::MockFunction<bool> mock_read_;
  mock_function::MockFunction<void, fuchsia_hardware_gpio::BufferMode> mock_set_buffer_mode_;
  mock_function::MockFunction<void, uint64_t> mock_set_drive_strength_;
  mock_function::MockFunction<uint64_t> mock_get_drive_strength_;
  mock_function::MockFunction<void> mock_get_interrupt_;
  mock_function::MockFunction<void, fuchsia_hardware_gpio::InterruptMode> mock_configure_interrupt_;
  mock_function::MockFunction<void> mock_release_interrupt_;
  mock_function::MockFunction<void, uint64_t> mock_set_function_;
  mock_function::MockFunction<void, std::string> mock_set_function_name_;
  mock_function::MockFunction<void, fuchsia_hardware_pin::Pull> mock_set_pull_;
  bool client_got_interrupt_ = false;
};

class GpioUtilTest : public zxtest::Test {
 public:
  void SetUp() override {
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);
    gpio_ = std::make_unique<FakeGpio>(loop_->dispatcher());

    zx::result server = fidl::CreateEndpoints(&client_);
    ASSERT_OK(server.status_value());
    fidl::BindServer(loop_->dispatcher(), std::move(server.value()), gpio_.get());

    ASSERT_OK(loop_->StartThread("gpioutil-test-loop"));
  }

  void TearDown() override {
    gpio_->MockGetPin().VerifyAndClear();
    gpio_->MockGetName().VerifyAndClear();
    gpio_->MockRead().VerifyAndClear();
    gpio_->MockSetBufferMode().VerifyAndClear();
    gpio_->MockSetDriveStrength().VerifyAndClear();
    gpio_->MockGetDriveStrength().VerifyAndClear();
    gpio_->MockGetInterrupt().VerifyAndClear();
    gpio_->MockConfigureInterrupt().VerifyAndClear();
    gpio_->MockReleaseInterrupt().VerifyAndClear();
    gpio_->MockSetFunction().VerifyAndClear();
    gpio_->MockSetPull().VerifyAndClear();

    loop_->Shutdown();
  }

 protected:
  std::unique_ptr<async::Loop> loop_;
  fidl::ClientEnd<fuchsia_hardware_pin::Debug> client_;
  std::unique_ptr<FakeGpio> gpio_;
};

TEST_F(GpioUtilTest, GetNameTest) {
  int argc = 3;
  const char* argv[] = {"gpioutil", "n", "some_path"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, GetName);

  gpio_->MockGetPin().ExpectCall();
  gpio_->MockGetName().ExpectCall();
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, ReadTest) {
  int argc = 3;
  const char* argv[] = {"gpioutil", "r", "some_path"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, Read);

  gpio_->MockRead().ExpectCall(true);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, InTest) {
  int argc = 3;
  const char* argv[] = {"gpioutil", "i", "some_path"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, SetBufferMode);
  EXPECT_EQ(buffer_mode, fuchsia_hardware_gpio::BufferMode::kInput);

  gpio_->MockSetBufferMode().ExpectCall(fuchsia_hardware_gpio::BufferMode::kInput);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, OutTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "o", "some_path", "3"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, SetBufferMode);
  EXPECT_EQ(buffer_mode, fuchsia_hardware_gpio::BufferMode::kOutputHigh);

  gpio_->MockSetBufferMode().ExpectCall(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, SetDriveStrengthTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "d", "some_path", "2000"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, Configure);
  ASSERT_TRUE(config.has_drive_strength_ua());
  EXPECT_EQ(config.drive_strength_ua(), 2'000);

  gpio_->MockSetDriveStrength().ExpectCall(2'000);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, GetDriveStrengthTest) {
  int argc = 3;
  const char* argv[] = {"gpioutil", "d", "some_path"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, GetDriveStrength);

  gpio_->MockGetDriveStrength().ExpectCall(3'000);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, InterruptTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "q", "some_path", "level-low"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, Interrupt);
  EXPECT_EQ(interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kLevelLow);

  gpio_->MockGetInterrupt().ExpectCall();
  gpio_->MockConfigureInterrupt().ExpectCall(fuchsia_hardware_gpio::InterruptMode::kLevelLow);
  gpio_->MockReleaseInterrupt().ExpectCall();
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, AltFunctionTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "f", "some_path", "6"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, Configure);
  ASSERT_TRUE(config.has_function());
  EXPECT_EQ(config.function(), 6);

  gpio_->MockSetFunction().ExpectCall(6);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, FunctionByNameTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "f", "some_path", "spi"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, Configure);
  ASSERT_TRUE(config.has_function_name());
  EXPECT_EQ(config.function_name().get(), "spi");

  gpio_->MockSetFunctionName().ExpectCall("spi");
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

TEST_F(GpioUtilTest, PullTest) {
  int argc = 4;
  const char* argv[] = {"gpioutil", "p", "some_path", "up"};

  fidl::Arena arena;
  GpioFunc func;
  fuchsia_hardware_gpio::BufferMode buffer_mode;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode;
  fuchsia_hardware_pin::wire::Configuration config;
  EXPECT_EQ(ParseArgs(argc, const_cast<char**>(argv), &func, arena, &buffer_mode, &interrupt_mode,
                      &config),
            0);
  EXPECT_EQ(func, Configure);
  ASSERT_TRUE(config.has_pull());
  EXPECT_EQ(config.pull(), fuchsia_hardware_pin::Pull::kUp);

  gpio_->MockSetPull().ExpectCall(fuchsia_hardware_pin::Pull::kUp);
  EXPECT_EQ(ClientCall(fidl::WireSyncClient(std::move(client_)), func, arena, buffer_mode,
                       interrupt_mode, config),
            0);
}

}  // namespace
