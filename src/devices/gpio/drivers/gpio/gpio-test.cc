// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gpio.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pinimpl/cpp/driver/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <format>
#include <optional>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <gtest/gtest.h>
#include <sdk/lib/driver/testing/cpp/driver_runtime.h>
#include <src/lib/testing/predicates/status.h>

#include "src/devices/gpio/drivers/gpio/gpio_config.h"

#define DECL_GPIO_PIN(x)     \
  {                          \
    {                        \
      .pin = (x), .name = #x \
    }                        \
  }

namespace gpio {

namespace {

class GenericMetadataServer final : public fidl::WireServer<fuchsia_driver_metadata::Metadata> {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher,
                     std::string service_name, fuchsia_driver_metadata::Dictionary metadata) {
    auto persisted = fidl::Persist(metadata);
    if (persisted.is_error()) {
      return zx::error(persisted.error_value().status());
    }
    metadata_bytes_ = std::move(persisted.value());

    fuchsia_driver_metadata::Service::InstanceHandler handler({
        .metadata = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });

    return outgoing.component().AddService(std::move(handler), std::move(service_name));
  }

 private:
  void GetPersistedMetadata(GetPersistedMetadataCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(metadata_bytes_));
  }

  std::vector<uint8_t> metadata_bytes_;
  fidl::ServerBindingGroup<fuchsia_driver_metadata::Metadata> bindings_;
};

class MockPinImpl : public fdf::WireServer<fuchsia_hardware_pinimpl::PinImpl> {
 public:
  static constexpr uint32_t kMaxInitStepPinIndex = 10;

  struct PinState {
    enum Mode { kUnknown, kIn, kOut };

    Mode mode = kUnknown;
    bool value = false;
    fuchsia_hardware_pin::Pull pull;
    uint64_t alt_function = UINT64_MAX;
    uint64_t drive_strength = UINT64_MAX;
    fuchsia_hardware_gpio::InterruptMode interrupt_mode;
    bool has_interrupt = false;
  };

  PinState pin_state(uint32_t index) { return pin_state_internal(index); }
  void set_pin_state(uint32_t index, PinState state) { pin_state_internal(index) = state; }

  zx_status_t Serve(fdf::OutgoingDirectory& to_driver_vfs) {
    fuchsia_hardware_pinimpl::Service::InstanceHandler instance_handler({
        .device =
            [this](fdf::ServerEnd<fuchsia_hardware_pinimpl::PinImpl> server) {
              EXPECT_FALSE(binding_);
              binding_.emplace(fdf::Dispatcher::GetCurrent()->get(), std::move(server), this,
                               fidl::kIgnoreBindingClosure);
            },
    });
    return to_driver_vfs.AddService<fuchsia_hardware_pinimpl::Service>(std::move(instance_handler))
        .status_value();
  }

 private:
  PinState& pin_state_internal(uint32_t index) {
    if (index >= pins_.size()) {
      pins_.resize(index + 1);
    }
    return pins_[index];
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_pinimpl::PinImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void Read(fuchsia_hardware_pinimpl::wire::PinImplReadRequest* request, fdf::Arena& arena,
            ReadCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess(pin_state_internal(request->pin).value);
  }

  void SetBufferMode(fuchsia_hardware_pinimpl::wire::PinImplSetBufferModeRequest* request,
                     fdf::Arena& arena, SetBufferModeCompleter::Sync& completer) override {
    if (request->pin > kMaxInitStepPinIndex) {
      return completer.buffer(arena).ReplyError(ZX_ERR_NOT_FOUND);
    }

    switch (request->mode) {
      case fuchsia_hardware_gpio::BufferMode::kOutputLow:
        pin_state_internal(request->pin).value = false;
        pin_state_internal(request->pin).mode = PinState::Mode::kOut;
        break;
      case fuchsia_hardware_gpio::BufferMode::kOutputHigh:
        pin_state_internal(request->pin).value = true;
        pin_state_internal(request->pin).mode = PinState::Mode::kOut;
        break;
      case fuchsia_hardware_gpio::BufferMode::kInput:
        pin_state_internal(request->pin).mode = PinState::Mode::kIn;
        break;
      default:
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
        return;
    }

    completer.buffer(arena).ReplySuccess();
  }

  void GetInterrupt(fuchsia_hardware_pinimpl::wire::PinImplGetInterruptRequest* request,
                    fdf::Arena& arena, GetInterruptCompleter::Sync& completer) override {
    if (pin_state_internal(request->pin).has_interrupt) {
      FAIL() << "GetInterrupt() called with interrupt already outstanding";
      completer.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
    }

    zx::interrupt ret;
    ASSERT_OK(zx::interrupt::create({}, {}, ZX_INTERRUPT_VIRTUAL, &ret));

    pin_state_internal(request->pin).has_interrupt = true;
    completer.buffer(arena).ReplySuccess(std::move(ret));
  }

  void ConfigureInterrupt(fuchsia_hardware_pinimpl::wire::PinImplConfigureInterruptRequest* request,
                          fdf::Arena& arena,
                          ConfigureInterruptCompleter::Sync& completer) override {
    if (request->config.has_mode()) {
      pin_state_internal(request->pin).interrupt_mode = request->config.mode();
      completer.buffer(arena).ReplySuccess();
    } else {
      completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    }
  }

  void ReleaseInterrupt(fuchsia_hardware_pinimpl::wire::PinImplReleaseInterruptRequest* request,
                        fdf::Arena& arena, ReleaseInterruptCompleter::Sync& completer) override {
    if (pin_state_internal(request->pin).has_interrupt) {
      pin_state_internal(request->pin).has_interrupt = false;
      completer.buffer(arena).ReplySuccess();
    } else {
      FAIL() << "ReleaseInterrupt() called with no outstanding interrupt";
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_FOUND);
    }
  }

  void Configure(fuchsia_hardware_pinimpl::wire::PinImplConfigureRequest* request,
                 fdf::Arena& arena, ConfigureCompleter::Sync& completer) override {
    if (request->pin > kMaxInitStepPinIndex) {
      return completer.buffer(arena).ReplyError(ZX_ERR_NOT_FOUND);
    }

    if (request->config.has_pull()) {
      pin_state_internal(request->pin).pull = request->config.pull();
    }

    if (request->config.has_function()) {
      pin_state_internal(request->pin).alt_function = request->config.function();
    }

    if (request->config.has_drive_strength_ua()) {
      pin_state_internal(request->pin).drive_strength = request->config.drive_strength_ua();
    }

    auto new_config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                          .pull(pin_state_internal(request->pin).pull)
                          .function(pin_state_internal(request->pin).alt_function)
                          .drive_strength_ua(pin_state_internal(request->pin).drive_strength)
                          .Build();
    completer.buffer(arena).ReplySuccess(new_config);
  }

  std::optional<fdf::ServerBinding<fuchsia_hardware_pinimpl::PinImpl>> binding_;
  std::vector<PinState> pins_;
};

class GpioTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    EXPECT_OK(pinimpl_.Serve(to_driver_vfs));
    if (pin_metadata_.has_value()) {
      EXPECT_OK(pin_metadata_server_.Serve(
          to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(), pin_metadata_.value()));
    }
    if (generic_metadata_.has_value()) {
      if (zx::result result = generic_metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              "fuchsia.hardware.pinimpl.Metadata", std::move(generic_metadata_.value()));
          result.is_error()) {
        return result.take_error();
      }
    }
    if (role_name_.has_value()) {
      EXPECT_OK(scheduler_role_name_metadata_server_.Serve(
          to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(), role_name_.value()));
    }
    return zx::ok();
  }

  void SetPinMetadata(fuchsia_hardware_pinimpl::Metadata metadata) {
    pin_metadata_ = std::move(metadata);
  }

  void InitGeneric(std::vector<fuchsia_hardware_pinimpl::Pin> pins) {
    std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
    entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
        "pins._count",
        fuchsia_driver_metadata::DictionaryValue::WithInt64(static_cast<int64_t>(pins.size()))));
    for (size_t i = 0; i < pins.size(); ++i) {
      const auto& pin = pins[i];
      if (pin.pin().has_value()) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            std::format("pins.{}.pin", i), fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                               static_cast<int64_t>(pin.pin().value()))));
      }
      if (pin.name().has_value()) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            std::format("pins.{}.name", i),
            fuchsia_driver_metadata::DictionaryValue::WithStr(pin.name().value())));
      }
      if (pin.id().has_value()) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            std::format("pins.{}.id", i), fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                              static_cast<int64_t>(pin.id().value()))));
      }
    }
    generic_metadata_ = fuchsia_driver_metadata::Dictionary{{.entries = std::move(entries)}};
  }

  void SetSchedulerRoleName(fuchsia_scheduler::RoleName role_name) {
    role_name_ = std::move(role_name);
  }

  MockPinImpl& pinimpl() { return pinimpl_; }

 private:
  MockPinImpl pinimpl_;
  fdf_metadata::MetadataServer<fuchsia_hardware_pinimpl::Metadata> pin_metadata_server_;
  fdf_metadata::MetadataServer<fuchsia_scheduler::RoleName> scheduler_role_name_metadata_server_;
  GenericMetadataServer generic_metadata_server_;
  std::optional<fuchsia_hardware_pinimpl::Metadata> pin_metadata_;
  std::optional<fuchsia_scheduler::RoleName> role_name_;
  std::optional<fuchsia_driver_metadata::Dictionary> generic_metadata_;
};

class FixtureConfig final {
 public:
  using DriverType = GpioRootDevice;
  using EnvironmentType = GpioTestEnvironment;
};

class GpioTest : public ::testing::Test {
 public:
  MockPinImpl::PinState pin_state(uint32_t index) {
    return driver_test().RunInEnvironmentTypeContext<MockPinImpl::PinState>(
        [index](GpioTestEnvironment& env) { return env.pinimpl().pin_state(index); });
  }

  void set_pin_state(uint32_t index, MockPinImpl::PinState state) {
    driver_test().RunInEnvironmentTypeContext(
        [index, state](GpioTestEnvironment& env) { env.pinimpl().set_pin_state(index, state); });
  }

  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 protected:
  void SetPinMetadata(const fuchsia_hardware_pinimpl::Metadata& metadata) {
    driver_test().RunInEnvironmentTypeContext(
        [&](GpioTestEnvironment& env) { env.SetPinMetadata(metadata); });
  }

  void SetGenericPinMetadata(std::vector<fuchsia_hardware_pinimpl::Pin> pins) {
    driver_test().RunInEnvironmentTypeContext(
        [pins = std::move(pins)](GpioTestEnvironment& env) mutable {
          env.InitGeneric(std::move(pins));
        });
  }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(GpioTest, TestGpioAll) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                       DECL_GPIO_PIN(2),
                       DECL_GPIO_PIN(3),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  set_pin_state(1, MockPinImpl::PinState{.value = true});
  gpio_client->Read().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::Read>& result) {
        ASSERT_OK(result.status());
        EXPECT_TRUE(result->value()->value);
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  gpio_client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::SetBufferMode>& result) {
            EXPECT_OK(result.status());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_TRUE(pin_state(1).value);

  gpio_client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::SetBufferMode>& result) {
            EXPECT_OK(result.status());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).mode, MockPinImpl::PinState::kIn);

  gpio_client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::SetBufferMode>& result) {
            EXPECT_OK(result.status());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).mode, MockPinImpl::PinState::kOut);
  EXPECT_TRUE(pin_state(1).value);

  fidl::Arena arena;
  auto interrupt_config = [&arena](fuchsia_hardware_gpio::InterruptMode mode) {
    return fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena).mode(mode).Build();
  };

  gpio_client->ConfigureInterrupt(interrupt_config(fuchsia_hardware_gpio::InterruptMode::kEdgeHigh))
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_OK(result.status());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kEdgeHigh);

  gpio_client->ConfigureInterrupt(interrupt_config(fuchsia_hardware_gpio::InterruptMode::kEdgeLow))
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_OK(result.status());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kEdgeLow);

  gpio_client
      ->ConfigureInterrupt(interrupt_config(fuchsia_hardware_gpio::InterruptMode::kLevelHigh))
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_OK(result.status());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kLevelHigh);

  gpio_client->ConfigureInterrupt(interrupt_config(fuchsia_hardware_gpio::InterruptMode::kLevelLow))
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_OK(result.status());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kLevelLow);

  gpio_client->ConfigureInterrupt(interrupt_config(fuchsia_hardware_gpio::InterruptMode::kEdgeBoth))
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_OK(result.status());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kEdgeBoth);

  gpio_client->ConfigureInterrupt({}).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
        ASSERT_OK(result.status());
        // The mock pinimpl driver should return an error for the empty interrupt config.
        EXPECT_TRUE(result->is_error());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kEdgeBoth);

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, TestPinAll) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                       DECL_GPIO_PIN(2),
                       DECL_GPIO_PIN(3),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  zx::result client_end = driver_test().Connect<fuchsia_hardware_pin::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_pin::Pin> pin_client(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  set_pin_state(1, MockPinImpl::PinState{
                       .pull = fuchsia_hardware_pin::Pull::kNone,
                       .alt_function = 0,
                       .drive_strength = 1000,
                   });

  fdf::Arena arena('TEST');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kUp)
                    .function(1)
                    .drive_strength_ua(2000)
                    .Build();
  pin_client->Configure(config).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_pin::Pin::Configure>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());

        ASSERT_TRUE(result->value()->new_config.has_pull());
        EXPECT_EQ(result->value()->new_config.pull(), fuchsia_hardware_pin::Pull::kUp);

        ASSERT_TRUE(result->value()->new_config.has_function());
        EXPECT_EQ(result->value()->new_config.function(), 1u);

        ASSERT_TRUE(result->value()->new_config.has_drive_strength_ua());
        EXPECT_EQ(result->value()->new_config.drive_strength_ua(), 2000u);
      });

  pin_client->Configure({}).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_pin::Pin::Configure>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());

        ASSERT_TRUE(result->value()->new_config.has_drive_strength_ua());
        EXPECT_EQ(result->value()->new_config.drive_strength_ua(), 2000u);

        driver_test().runtime().Quit();
      });

  driver_test().runtime().Run();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, ValidateMetadataOk) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                       DECL_GPIO_PIN(2),
                       DECL_GPIO_PIN(3),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(node.children().count("gpio"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-1"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-1-pin"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-2"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-2-pin"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-3"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-3-pin"), 1ul);
  });

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, ValidateMetadataRejectDuplicates) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(2),
                       DECL_GPIO_PIN(1),
                       DECL_GPIO_PIN(2),
                       DECL_GPIO_PIN(0),
                   }}}});

  ASSERT_FALSE(driver_test()
                   .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                     gpio_config::Config config{{.enable_suspend = true}};
                     args.config(config.ToVmo());
                   })
                   .is_ok());
}

TEST_F(GpioTest, ValidateGpioNameGeneration) {
  const fuchsia_hardware_pinimpl::Pin pins_digit[] = {
      DECL_GPIO_PIN(2),
      DECL_GPIO_PIN(5),
      DECL_GPIO_PIN((11)),
  };
  EXPECT_EQ(pins_digit[0].pin().value(), 2u);
  EXPECT_EQ(pins_digit[0].name().value(), "2");
  EXPECT_EQ(pins_digit[1].pin().value(), 5u);
  EXPECT_EQ(pins_digit[1].name().value(), "5");
  EXPECT_EQ(pins_digit[2].pin().value(), 11u);
  EXPECT_EQ(pins_digit[2].name().value(), "(11)");

#define GPIO_TEST_NAME1 5
#define GPIO_TEST_NAME2 (6)
#define GPIO_TEST_NAME3_OF_63_CHRS_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890 7
  constexpr uint32_t GPIO_TEST_NAME4 = 8;  // constexpr should work too
#define GEN_GPIO0(x) ((x) + 1)
#define GEN_GPIO1(x) ((x) + 2)
  const fuchsia_hardware_pinimpl::Pin pins[] = {
      DECL_GPIO_PIN(GPIO_TEST_NAME1),
      DECL_GPIO_PIN(GPIO_TEST_NAME2),
      DECL_GPIO_PIN(GPIO_TEST_NAME3_OF_63_CHRS_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890),
      DECL_GPIO_PIN(GPIO_TEST_NAME4),
      DECL_GPIO_PIN(GEN_GPIO0(9)),
      DECL_GPIO_PIN(GEN_GPIO1(18)),
  };
  EXPECT_EQ(pins[0].pin().value(), 5u);
  EXPECT_EQ(pins[0].name().value(), "GPIO_TEST_NAME1");
  EXPECT_EQ(pins[1].pin().value(), 6u);
  EXPECT_EQ(pins[1].name().value(), "GPIO_TEST_NAME2");
  EXPECT_EQ(pins[2].pin().value(), 7u);
  EXPECT_EQ(pins[2].name().value(),
            "GPIO_TEST_NAME3_OF_63_CHRS_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890");
  EXPECT_EQ(pins[2].name().value().size(), fuchsia_hardware_pin::kMaxPinNameLen - 1ul);
  EXPECT_EQ(pins[3].pin().value(), 8u);
  EXPECT_EQ(pins[3].name().value(), "GPIO_TEST_NAME4");
  EXPECT_EQ(pins[4].pin().value(), 10u);
  EXPECT_EQ(pins[4].name().value(), "GEN_GPIO0(9)");
  EXPECT_EQ(pins[5].pin().value(), 20u);
  EXPECT_EQ(pins[5].name().value(), "GEN_GPIO1(18)");
#undef GPIO_TEST_NAME1
#undef GPIO_TEST_NAME2
#undef GPIO_TEST_NAME3_OF_63_CHRS_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890
#undef GEN_GPIO0
#undef GEN_GPIO1
}

TEST_F(GpioTest, Init) {
  namespace fhgpio = fuchsia_hardware_gpio;
  namespace fhpin = fuchsia_hardware_pin;
  namespace fhpinimpl = fuchsia_hardware_pinimpl;

  auto mode = [](uint32_t pin, fhgpio::BufferMode buffer_mode) {
    return fhpinimpl::InitStep::WithCall({{pin, fhpinimpl::InitCall::WithBufferMode(buffer_mode)}});
  };

  auto config = [](uint32_t pin, fhpin::Configuration config) {
    return fhpinimpl::InitStep::WithCall(
        {{pin, fhpinimpl::InitCall::WithPinConfig(std::move(config))}});
  };

  std::vector<fhpinimpl::InitStep> steps;

  steps.push_back(config(1, fhpin::Configuration{{.pull = fhpin::Pull::kDown}}));
  steps.push_back(mode(1, fhgpio::BufferMode::kOutputHigh));
  steps.push_back(config(1, fhpin::Configuration{{.drive_strength_ua = 4000}}));
  steps.push_back(config(2, fhpin::Configuration{{.pull = fhpin::Pull::kNone}}));
  steps.push_back(config(2, fhpin::Configuration{{.function = 5}}));
  steps.push_back(config(2, fhpin::Configuration{{.drive_strength_ua = 2000}}));
  steps.push_back(mode(3, fhgpio::BufferMode::kOutputLow));
  steps.push_back(mode(3, fhgpio::BufferMode::kOutputHigh));
  steps.push_back(config(3, fhpin::Configuration{{.pull = fhpin::Pull::kUp}}));
  steps.push_back(config(2, fhpin::Configuration{{.function = 0}}));
  steps.push_back(config(2, fhpin::Configuration{{.drive_strength_ua = 1000}}));
  steps.push_back(mode(2, fhgpio::BufferMode::kOutputHigh));
  steps.push_back(config(1, fhpin::Configuration{{.pull = fhpin::Pull::kUp}}));
  steps.push_back(config(1, fhpin::Configuration{{.function = 0}}));
  steps.push_back(config(1, fhpin::Configuration{{.drive_strength_ua = 4000}}));
  steps.push_back(mode(1, fhgpio::BufferMode::kOutputHigh));
  steps.push_back(config(3, fhpin::Configuration{{.function = 3}}));
  steps.push_back(config(3, fhpin::Configuration{{.drive_strength_ua = 2000}}));

  SetPinMetadata({{
      .init_steps = std::move(steps),
      .pins = {{
          DECL_GPIO_PIN(1),
          DECL_GPIO_PIN(2),
          DECL_GPIO_PIN(3),
      }},
  }});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  // Validate the final state of the pins with the init steps applied.
  EXPECT_EQ(pin_state(1).mode, MockPinImpl::PinState::Mode::kOut);
  EXPECT_EQ(pin_state(1).pull, fuchsia_hardware_pin::Pull::kUp);
  EXPECT_TRUE(pin_state(1).value);
  EXPECT_EQ(pin_state(1).alt_function, 0ul);
  EXPECT_EQ(pin_state(1).drive_strength, 4000ul);

  EXPECT_EQ(pin_state(2).mode, MockPinImpl::PinState::Mode::kOut);
  EXPECT_EQ(pin_state(2).pull, fuchsia_hardware_pin::Pull::kNone);
  EXPECT_TRUE(pin_state(2).value);
  EXPECT_EQ(pin_state(2).alt_function, 0ul);
  EXPECT_EQ(pin_state(2).drive_strength, 1000ul);

  EXPECT_EQ(pin_state(3).mode, MockPinImpl::PinState::Mode::kOut);
  EXPECT_EQ(pin_state(3).pull, fuchsia_hardware_pin::Pull::kUp);
  EXPECT_TRUE(pin_state(3).value);
  EXPECT_EQ(pin_state(3).alt_function, 3ul);
  EXPECT_EQ(pin_state(3).drive_strength, 2000ul);

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, InitWithoutPins) {
  namespace fhpin = fuchsia_hardware_pin;
  namespace fhpinimpl = fuchsia_hardware_pinimpl;

  std::vector<fhpinimpl::InitStep> steps;
  steps.push_back(fhpinimpl::InitStep::WithCall(
      {{1, fhpinimpl::InitCall::WithPinConfig({{.pull = fhpin::Pull::kDown}})}}));

  SetPinMetadata({{.init_steps = std::move(steps)}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  EXPECT_EQ(pin_state(1).pull, fuchsia_hardware_pin::Pull::kDown);

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    EXPECT_EQ(node.children().count("gpio"), 1ul);
    EXPECT_EQ(node.children().count("gpio-init"), 1ul);
  });

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, InitErrorHandling) {
  namespace fhgpio = fuchsia_hardware_gpio;
  namespace fhpin = fuchsia_hardware_pin;
  namespace fhpinimpl = fuchsia_hardware_pinimpl;

  auto mode = [](uint32_t pin, fhgpio::BufferMode buffer_mode) {
    return fhpinimpl::InitStep::WithCall({{pin, fhpinimpl::InitCall::WithBufferMode(buffer_mode)}});
  };

  auto config = [](uint32_t pin, fhpin::Configuration config) {
    return fhpinimpl::InitStep::WithCall(
        {{pin, fhpinimpl::InitCall::WithPinConfig(std::move(config))}});
  };

  std::vector<fhpinimpl::InitStep> steps;

  steps.push_back(config(4, fhpin::Configuration{{.pull = fhpin::Pull::kDown}}));
  steps.push_back(mode(4, fhgpio::BufferMode::kOutputHigh));
  steps.push_back(config(4, fhpin::Configuration{{.drive_strength_ua = 4000}}));
  steps.push_back(config(2, fhpin::Configuration{{.pull = fhpin::Pull::kNone}}));
  steps.push_back(config(2, fhpin::Configuration{{.function = 5}}));
  steps.push_back(config(2, fhpin::Configuration{{.drive_strength_ua = 2000}}));

  // Using an index of 11 should cause the fake pinimpl device to return an error.
  steps.push_back(mode(MockPinImpl::kMaxInitStepPinIndex + 1, fhgpio::BufferMode::kOutputLow));

  // Processing should not continue after the above error.

  steps.push_back(config(2, fhpin::Configuration{{.function = 0}}));
  steps.push_back(config(2, fhpin::Configuration{{.drive_strength_ua = 1000}}));

  SetPinMetadata({{
      .init_steps = std::move(steps),
      .pins = {{
          DECL_GPIO_PIN(1),
          DECL_GPIO_PIN(2),
          DECL_GPIO_PIN(3),
      }},
  }});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  EXPECT_EQ(pin_state(2).mode, MockPinImpl::PinState::Mode::kUnknown);
  EXPECT_EQ(pin_state(2).pull, fuchsia_hardware_pin::Pull::kNone);
  EXPECT_EQ(pin_state(2).alt_function, 5ul);
  EXPECT_EQ(pin_state(2).drive_strength, 2000ul);

  EXPECT_EQ(pin_state(4).mode, MockPinImpl::PinState::Mode::kOut);
  EXPECT_EQ(pin_state(4).pull, fuchsia_hardware_pin::Pull::kDown);
  EXPECT_TRUE(pin_state(4).value);
  EXPECT_EQ(pin_state(4).drive_strength, 4000ul);

  // GPIO root device (init device should not be added due to errors).
  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    EXPECT_EQ(node.children().count("gpio"), 1ul);
    EXPECT_EQ(node.children().count("gpio-init"), 0ul);
  });

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, ControllerId) {
  constexpr uint32_t kController = 5;

  static const std::vector<fuchsia_hardware_pinimpl::Pin> kPins = {
      DECL_GPIO_PIN(0),
      DECL_GPIO_PIN(1),
      DECL_GPIO_PIN(2),
  };

  SetPinMetadata({{
      .controller_id = kController,
      .pins = kPins,
  }});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(node.children().count("gpio"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-0"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-0-pin"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-1"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-1-pin"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-2"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-2-pin"), 1ul);
  });

  for (const auto& pin : kPins) {
    driver_test().RunInNodeContext([&](fdf_testing::TestNode& node) {
      ASSERT_EQ(
          node.children().at("gpio").children().count(std::string{"gpio-"} + pin.name().value()),
          1ul);
      std::vector<fuchsia_driver_framework::NodeProperty2> properties =
          node.children()
              .at("gpio")
              .children()
              .at(std::string{"gpio-"} + pin.name().value())
              .GetProperties();

      ASSERT_EQ(properties.size(), 3ul);

      EXPECT_EQ(properties[0].key(), bind_fuchsia::ID);

      ASSERT_TRUE(properties[0].value().int_value().has_value());
      EXPECT_EQ(properties[0].value().int_value().value(), pin.pin().value());

      EXPECT_EQ(properties[1].key(), bind_fuchsia::NAME);

      ASSERT_TRUE(properties[1].value().string_value().has_value());
      EXPECT_EQ(properties[1].value().string_value().value(),
                std::string{"gpio-"} + pin.name().value());

      EXPECT_EQ(properties[2].key(), bind_fuchsia::SERVICE);
      ASSERT_TRUE(properties[2].value().string_value().has_value());
      EXPECT_EQ(properties[2].value().string_value().value(), fuchsia_hardware_gpio::Service::Name);

      std::vector<fuchsia_driver_framework::NodeProperty2> pin_properties =
          node.children()
              .at("gpio")
              .children()
              .at(std::string{"gpio-"} + pin.name().value() + "-pin")
              .GetProperties();

      ASSERT_EQ(pin_properties.size(), 3ul);
      EXPECT_EQ(pin_properties[0].key(), bind_fuchsia::ID);
      ASSERT_TRUE(pin_properties[0].value().int_value().has_value());
      EXPECT_EQ(pin_properties[0].value().int_value().value(), pin.pin().value());
      EXPECT_EQ(pin_properties[1].key(), bind_fuchsia::NAME);
      ASSERT_TRUE(pin_properties[1].value().string_value().has_value());
      EXPECT_EQ(pin_properties[1].value().string_value().value(),
                std::string{"gpio-"} + pin.name().value());
      EXPECT_EQ(pin_properties[2].key(), bind_fuchsia::SERVICE);
      ASSERT_TRUE(pin_properties[2].value().string_value().has_value());
      EXPECT_EQ(pin_properties[2].value().string_value().value(),
                fuchsia_hardware_pin::Service::Name);
    });
  }

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, SchedulerRole) {
  static const std::vector<fuchsia_hardware_pinimpl::Pin> kPins = {
      DECL_GPIO_PIN(0),
      DECL_GPIO_PIN(1),
      DECL_GPIO_PIN(2),
  };
  SetPinMetadata({{.pins = kPins}});

  driver_test().RunInEnvironmentTypeContext([&](GpioTestEnvironment& env) {
    // Add scheduler role metadata that will cause the core driver to create a new driver
    // dispatcher. Verify that FIDL calls can still be made, and that dispatcher shutdown using the
    // unbind hook works.
    fuchsia_scheduler::RoleName kRoleName("no.such.scheduler.role");
    env.SetSchedulerRoleName(kRoleName);
  });

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  // Wait for children to be added.
  bool node_added = false;
  for (int i = 0; i < 100 && !node_added; ++i) {
    node_added = driver_test().RunInNodeContext(
        fit::callback<bool(fdf_testing::TestNode&)>([&](fdf_testing::TestNode& node) {
          return node.children().contains("gpio") &&
                 node.children().at("gpio").children().contains("gpio-0");
        }));
    if (!node_added) {
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }
  ASSERT_TRUE(node_added);

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(node.children().count("gpio"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-0"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-0-pin"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-1"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-1-pin"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-2"), 1ul);
    EXPECT_EQ(node.children().at("gpio").children().count("gpio-2-pin"), 1ul);
  });

  for (const auto& pin : kPins) {
    zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>(
        std::string{"gpio-"} + pin.name().value());
    EXPECT_TRUE(client_end.is_ok());

    // Run the dispatcher to allow the connection to be established.
    driver_test().runtime().RunUntilIdle();

    // The GPIO driver should be bound on a different dispatcher, so a sync client should work here.
    fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> gpio_client(*std::move(client_end));
    auto result = gpio_client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, MultipleClients) {
  static const std::vector<fuchsia_hardware_pinimpl::Pin> kPins = {
      DECL_GPIO_PIN(0),
      DECL_GPIO_PIN(1),
      DECL_GPIO_PIN(2),
  };

  SetPinMetadata({{.pins = kPins}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  std::vector<fidl::WireClient<fuchsia_hardware_gpio::Gpio>> clients;
  for (size_t i = 0; i < kPins.size(); ++i) {
    zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>(
        std::string{"gpio-"} + kPins[i].name().value());
    EXPECT_TRUE(client_end.is_ok());
    clients.emplace_back(std::move(client_end.value()),
                         fdf::Dispatcher::GetCurrent()->async_dispatcher());
    clients.back()->Read().ThenExactlyOnce([&, quit = i == kPins.size() - 1](auto& result) {
      ASSERT_TRUE(result.ok());
      EXPECT_TRUE(result->is_ok());
      if (quit) {
        driver_test().runtime().Quit();
      }
    });
  }

  driver_test().runtime().Run();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, DebugDevfs) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                       DECL_GPIO_PIN(2),
                       DECL_GPIO_PIN(3),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{
                        {.enable_suspend = true, .expose_debug_capabilities = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  zx::result client_end = driver_test().ConnectThroughDevfs<fuchsia_hardware_pin::Debug>(
      std::vector<std::string>{"gpio", "gpio-1"});
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_pin::Debug> debug_client(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  set_pin_state(1, MockPinImpl::PinState{
                       .mode = MockPinImpl::PinState::kIn,
                       .value = false,
                       .pull = fuchsia_hardware_pin::Pull::kNone,
                       .alt_function = 0,
                       .drive_strength = 1000,
                   });

  debug_client->GetProperties().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_pin::Debug::GetProperties>& result) {
        ASSERT_TRUE(result.ok());

        ASSERT_TRUE(result->has_name());
        std::string name(result->name().get());
        EXPECT_STREQ(name.data(), "1");

        ASSERT_TRUE(result->has_pin());
        EXPECT_EQ(result->pin(), 1u);
      });

  auto [pin_client_end, pin_server_end] = fidl::Endpoints<fuchsia_hardware_pin::Pin>::Create();
  fidl::WireClient<fuchsia_hardware_pin::Pin> pin_client(
      std::move(pin_client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  debug_client->ConnectPin(std::move(pin_server_end))
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_pin::Debug::ConnectPin>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
          });

  fdf::Arena arena('TEST');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kUp)
                    .function(1)
                    .drive_strength_ua(2000)
                    .Build();
  pin_client->Configure(config).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_pin::Pin::Configure>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());

        ASSERT_TRUE(result->value()->new_config.has_pull());
        EXPECT_EQ(result->value()->new_config.pull(), fuchsia_hardware_pin::Pull::kUp);
        EXPECT_EQ(pin_state(1).pull, fuchsia_hardware_pin::Pull::kUp);

        ASSERT_TRUE(result->value()->new_config.has_function());
        EXPECT_EQ(result->value()->new_config.function(), 1u);
        EXPECT_EQ(pin_state(1).alt_function, 1u);

        ASSERT_TRUE(result->value()->new_config.has_drive_strength_ua());
        EXPECT_EQ(result->value()->new_config.drive_strength_ua(), 2000u);
        EXPECT_EQ(pin_state(1).drive_strength, 2000u);

        driver_test().runtime().Quit();
      });

  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  auto [gpio_client_end, gpio_server_end] = fidl::Endpoints<fuchsia_hardware_gpio::Gpio>::Create();
  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client(
      std::move(gpio_client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  debug_client->ConnectGpio(std::move(gpio_server_end))
      .ThenExactlyOnce(
          [](fidl::WireUnownedResult<fuchsia_hardware_pin::Debug::ConnectGpio>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
          });

  gpio_client->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::SetBufferMode>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
            EXPECT_EQ(pin_state(1).mode, MockPinImpl::PinState::kOut);
            EXPECT_TRUE(pin_state(1).value);

            driver_test().runtime().Quit();
          });

  driver_test().runtime().Run();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, MultipleClientsGetInterrupts) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                       DECL_GPIO_PIN(2),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client1(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  gpio_client1->GetInterrupt({}).ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_TRUE(result->value()->interrupt.is_valid());
      });
  driver_test().runtime().RunUntilIdle();

  client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client2(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Attempting to get an interrupt from a second client should fail.
  gpio_client2->GetInterrupt({}).ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_ACCESS_DENIED);
      });
  driver_test().runtime().RunUntilIdle();

  client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-2");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client3(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Interrupts for other pins should still be available.
  gpio_client3->GetInterrupt({}).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_TRUE(result->value()->interrupt.is_valid());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  // Release the interrupt from the first client. The second client should now be able to get the
  // interrupt.
  gpio_client1->ReleaseInterrupt().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ReleaseInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  gpio_client2->GetInterrupt({}).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_TRUE(result->value()->interrupt.is_valid());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, UnbindingClientReleasesInterrupt) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  EXPECT_FALSE(pin_state(1).has_interrupt);

  zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  {
    fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client(
        *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

    gpio_client->GetInterrupt({}).ThenExactlyOnce(
        [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result->is_ok());
          EXPECT_TRUE(result->value()->interrupt.is_valid());
          driver_test().runtime().Quit();
        });
    driver_test().runtime().Run();
    driver_test().runtime().ResetQuit();

    EXPECT_TRUE(pin_state(1).has_interrupt);
  }

  // Allow the core driver to process the client unbinding.
  driver_test().runtime().RunUntilIdle();

  client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Synchronize with the mock pinimpl driver so that we can check the interrupt state.
  gpio_client->Read().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::Read>& result) {
        EXPECT_TRUE(result.ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  // Verify that the core driver called ReleaseInterrupt() on the pinimpl driver.
  EXPECT_FALSE(pin_state(1).has_interrupt);

  // A new client should be able to get the interrupt again.
  gpio_client->GetInterrupt({}).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_TRUE(result->value()->interrupt.is_valid());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();

  EXPECT_TRUE(pin_state(1).has_interrupt);

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, OnlyClientWithInterruptCanConfigure) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  EXPECT_FALSE(pin_state(1).has_interrupt);

  zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client1(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client2(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  fidl::Arena arena;
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(fuchsia_hardware_gpio::InterruptMode::kLevelHigh)
                              .Build();

  // Both clients should be able to configure the interrupt initially.
  gpio_client1->ConfigureInterrupt(interrupt_config)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  gpio_client2->ConfigureInterrupt(interrupt_config)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kLevelHigh);

  gpio_client1->GetInterrupt({}).ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_TRUE(result->value()->interrupt.is_valid());
      });

  // The first client has the interrupt and should be able to configure it.
  interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                         .mode(fuchsia_hardware_gpio::InterruptMode::kLevelLow)
                         .Build();
  gpio_client1->ConfigureInterrupt(interrupt_config)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kLevelLow);

  // The second client shouldn't be able to configure the interrupt now.
  interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                         .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                         .Build();
  gpio_client2->ConfigureInterrupt(interrupt_config)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_error());
            EXPECT_EQ(result->error_value(), ZX_ERR_ACCESS_DENIED);
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kLevelLow);

  // Release the interrupt from the first client. The second client should be able to configure the
  // interrupt again.
  gpio_client1->ReleaseInterrupt().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ReleaseInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  gpio_client2->ConfigureInterrupt(interrupt_config)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ConfigureInterrupt>& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
            driver_test().runtime().Quit();
          });
  driver_test().runtime().Run();

  EXPECT_EQ(pin_state(1).interrupt_mode, fuchsia_hardware_gpio::InterruptMode::kEdgeLow);

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, DoubleGetInterruptAndRelease) {
  SetPinMetadata({{.pins = {{
                       DECL_GPIO_PIN(1),
                   }}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  EXPECT_FALSE(pin_state(1).has_interrupt);

  zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio_client(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  gpio_client->GetInterrupt({}).ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_TRUE(result->value()->interrupt.is_valid());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  gpio_client->GetInterrupt({}).ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::GetInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_ALREADY_EXISTS);
      });
  driver_test().runtime().RunUntilIdle();

  gpio_client->ReleaseInterrupt().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ReleaseInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  gpio_client->ReleaseInterrupt().ThenExactlyOnce(
      [](fidl::WireUnownedResult<fuchsia_hardware_gpio::Gpio::ReleaseInterrupt>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_NOT_FOUND);
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, TestPinStates) {
  fuchsia_hardware_pinimpl::DevicePinStates dev_states;
  dev_states.name() = "my-device";

  // default state: Configure Pin 1 to Pull Up
  fuchsia_hardware_pinimpl::PinState default_state;
  default_state.name() = "default";

  fuchsia_hardware_pinimpl::PinConfiguration pin1_default{{
      .pin = 1,
      .call =
          fuchsia_hardware_pinimpl::InitCall::WithPinConfig(fuchsia_hardware_pin::Configuration{{
              .pull = fuchsia_hardware_pin::Pull::kUp,
          }}),
  }};
  default_state.pins().push_back(std::move(pin1_default));
  dev_states.states().push_back(std::move(default_state));

  // sleep state: Configure Pin 1 to Pull Down
  fuchsia_hardware_pinimpl::PinState sleep_state;
  sleep_state.name() = "sleep";

  fuchsia_hardware_pinimpl::PinConfiguration pin1_sleep{{
      .pin = 1,
      .call =
          fuchsia_hardware_pinimpl::InitCall::WithPinConfig(fuchsia_hardware_pin::Configuration{{
              .pull = fuchsia_hardware_pin::Pull::kDown,
          }}),
  }};
  sleep_state.pins().push_back(std::move(pin1_sleep));
  dev_states.states().push_back(std::move(sleep_state));

  SetPinMetadata({{.device_pin_states = {{dev_states}}}});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  // Default state should have been applied automatically.
  EXPECT_EQ(pin_state(1).pull, fuchsia_hardware_pin::Pull::kUp);

  zx::result client_end =
      driver_test().Connect<fuchsia_hardware_pin::PinStatesService::Device>("my-device");
  ASSERT_TRUE(client_end.is_ok());

  fidl::WireClient<fuchsia_hardware_pin::PinStates> client(
      *std::move(client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Switch to sleep state.
  client->SelectState("sleep").ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_pin::PinStates::SelectState>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_EQ(pin_state(1).pull, fuchsia_hardware_pin::Pull::kDown);

  // Try invalid state.
  client->SelectState("invalid").ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_pin::PinStates::SelectState>& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_NOT_FOUND);
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

TEST_F(GpioTest, GenericMetadataTest) {
  SetGenericPinMetadata({{
      {{.pin = 1, .name = "pin-1", .id = 42}},
      {{.pin = 2, .name = "pin-2"}},
  }});

  EXPECT_TRUE(driver_test()
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    gpio_config::Config config{{.enable_suspend = false}};
                    args.config(config.ToVmo());
                  })
                  .is_ok());

  zx::result client_end = driver_test().Connect<fuchsia_hardware_gpio::Service::Device>("gpio-1");
  EXPECT_TRUE(client_end.is_ok());

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    std::vector<fuchsia_driver_framework::NodeProperty2> properties =
        node.children().at("gpio").children().at("gpio-1").GetProperties();
    ASSERT_EQ(properties.size(), 3ul);
    EXPECT_EQ(properties[0].key(), bind_fuchsia::ID);
    ASSERT_TRUE(properties[0].value().int_value().has_value());
    EXPECT_EQ(properties[0].value().int_value().value(), 42ul);

    std::vector<fuchsia_driver_framework::NodeProperty2> pin_properties =
        node.children().at("gpio").children().at("gpio-1-pin").GetProperties();
    ASSERT_EQ(pin_properties.size(), 3ul);
    EXPECT_EQ(pin_properties[0].key(), bind_fuchsia::ID);
    ASSERT_TRUE(pin_properties[0].value().int_value().has_value());
    EXPECT_EQ(pin_properties[0].value().int_value().value(), 42ul);

    std::vector<fuchsia_driver_framework::NodeProperty2> pin2_properties =
        node.children().at("gpio").children().at("gpio-2").GetProperties();
    ASSERT_EQ(pin2_properties.size(), 3ul);
    EXPECT_EQ(pin2_properties[0].key(), bind_fuchsia::ID);
    ASSERT_TRUE(pin2_properties[0].value().int_value().has_value());
    EXPECT_EQ(pin2_properties[0].value().int_value().value(), 2ul);
  });

  EXPECT_TRUE(driver_test().StopDriver().is_ok());
}

}  // namespace

}  // namespace gpio
