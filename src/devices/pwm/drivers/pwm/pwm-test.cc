// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pwm.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pwmimpl/cpp/driver/wire_test_base.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace pwm {

namespace {

class GenericMetadataServer final : public fidl::WireServer<fuchsia_driver_metadata::Metadata> {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher,
                     std::string service_name,
                     const fuchsia_driver_metadata::Dictionary& metadata) {
    fit::result persisted_metadata = fidl::Persist(metadata);
    if (persisted_metadata.is_error()) {
      return zx::error(persisted_metadata.error_value().status());
    }
    persisted_metadata_ = std::move(persisted_metadata.value());

    fuchsia_driver_metadata::Service::InstanceHandler handler(
        {.metadata = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure)});

    return outgoing.component().AddService(std::move(handler), std::move(service_name));
  }

  void GetPersistedMetadata(GetPersistedMetadataCompleter::Sync& completer) override {
    if (!persisted_metadata_.has_value()) {
      completer.ReplyError(ZX_ERR_NOT_FOUND);
      return;
    }
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(persisted_metadata_.value()));
  }

 private:
  fidl::ServerBindingGroup<fuchsia_driver_metadata::Metadata> bindings_;
  std::optional<std::vector<uint8_t>> persisted_metadata_;
};

struct FakeModeConfig {
  uint32_t mode;
};

class FakePwmImpl : public fidl::testing::WireTestBase<fuchsia_hardware_pwmimpl::PwmImpl> {
 public:
  FakePwmImpl() = default;

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  // fdf::WireServer<fuchsia_hardware_pwmimpl::PwmImpl> implementation.
  void GetConfig(GetConfigRequestView request, fdf::Arena& arena,
                 GetConfigCompleter::Sync& completer) override {
    get_config_count_++;
    completer.buffer(arena).ReplySuccess(fidl::ToWire(arena, config_));
  }

  void SetConfig(SetConfigRequestView request, fdf::Arena& arena,
                 SetConfigCompleter::Sync& completer) override {
    set_config_count_++;
    config_ = fidl::ToNatural(request->config);
    completer.buffer(arena).ReplySuccess();
  }

  void Enable(EnableRequestView request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override {
    enable_count_++;
    completer.buffer(arena).ReplySuccess();
  }

  void Disable(DisableRequestView request, fdf::Arena& arena,
               DisableCompleter::Sync& completer) override {
    disable_count_++;
    completer.buffer(arena).ReplySuccess();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_pwmimpl::PwmImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  // Accessors
  unsigned int GetConfigCount() const { return get_config_count_; }
  unsigned int SetConfigCount() const { return set_config_count_; }
  unsigned int EnableCount() const { return enable_count_; }
  unsigned int DisableCount() const { return disable_count_; }

 private:
  unsigned int get_config_count_ = 0;
  unsigned int set_config_count_ = 0;
  unsigned int enable_count_ = 0;
  unsigned int disable_count_ = 0;

  fuchsia_hardware_pwm::PwmConfig config_{{
      .polarity = false,
      .period_ns = 0,
      .duty_cycle = 0.0,
      .mode_config = std::vector<uint8_t>{0},
  }};
};

class PwmTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(fuchsia_hardware_pwm::PwmChannelsMetadata metadata) { metadata_ = std::move(metadata); }

  void InitGeneric(fuchsia_hardware_pwm::PwmChannelsMetadata metadata) {
    std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
    if (metadata.channels().has_value()) {
      const auto& channels = metadata.channels().value();
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          "channels._count", fuchsia_driver_metadata::DictionaryValue::WithInt64(channels.size())));
      for (size_t i = 0; i < channels.size(); ++i) {
        if (channels[i].id().has_value()) {
          entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
              std::format("channels.{}.channel", i),
              fuchsia_driver_metadata::DictionaryValue::WithInt64(channels[i].id().value())));
        }
        if (channels[i].period_ns().has_value()) {
          entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
              std::format("channels.{}.period_ns", i),
              fuchsia_driver_metadata::DictionaryValue::WithInt64(
                  channels[i].period_ns().value())));
        }
      }
    }
    generic_metadata_ = fuchsia_driver_metadata::Dictionary{{.entries = std::move(entries)}};
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    fdf_dispatcher_t* driver_dispatcher = fdf::Dispatcher::GetCurrent()->get();

    zx::result<> result = to_driver_vfs.AddService<fuchsia_hardware_pwmimpl::Service>(
        fuchsia_hardware_pwmimpl::Service::InstanceHandler({
            .device =
                bindings_.CreateHandler(&pwm_impl_, driver_dispatcher, fidl::kIgnoreBindingClosure),
        }));
    if (result.is_error()) {
      return result.take_error();
    }

    if (metadata_.has_value()) {
      if (zx::result result = metadata_server_.Serve(to_driver_vfs, dispatcher, metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (generic_metadata_.has_value()) {
      if (zx::result result = generic_metadata_server_.Serve(
              to_driver_vfs, dispatcher, "fuchsia.hardware.pwm.PwmChannelsMetadata",
              generic_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  FakePwmImpl& pwm_impl() { return pwm_impl_; }

 private:
  FakePwmImpl pwm_impl_;
  fdf::ServerBindingGroup<fuchsia_hardware_pwmimpl::PwmImpl> bindings_;
  fdf_metadata::MetadataServer<fuchsia_hardware_pwm::PwmChannelsMetadata> metadata_server_;
  std::optional<fuchsia_hardware_pwm::PwmChannelsMetadata> metadata_;
  GenericMetadataServer generic_metadata_server_;
  std::optional<fuchsia_driver_metadata::Dictionary> generic_metadata_;
};

class FixtureConfig final {
 public:
  using DriverType = Pwm;
  using EnvironmentType = PwmTestEnvironment;
};

class PwmTest : public ::testing::Test {
 protected:
  void SetUp() override {
    static const fuchsia_hardware_pwm::PwmChannelsMetadata kTestMetadataChannels{
        {.channels{{{{.id = 0}}}}}};

    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.Init(kTestMetadataChannels); });
    ASSERT_OK(driver_test_.StartDriver());

    zx::result pwm = driver_test_.Connect<fuchsia_hardware_pwm::Service::Pwm>("pwm-0");
    ASSERT_OK(pwm);
    pwm_.Bind(std::move(pwm.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  void WithPwmImpl(fit::callback<void(FakePwmImpl& pwm_impl)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.pwm_impl()); });
  }

  fidl::SyncClient<fuchsia_hardware_pwm::Pwm>& pwm() { return pwm_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fidl::SyncClient<fuchsia_hardware_pwm::Pwm> pwm_;
};

TEST_F(PwmTest, GetConfigTest) {
  EXPECT_OK(pwm()->GetConfig());
  EXPECT_OK(pwm()->GetConfig());  // Second time
}

TEST_F(PwmTest, SetConfigTest) {
  FakeModeConfig fake_mode{
      .mode = 0,
  };
  const auto* fake_mode_bytes = reinterpret_cast<uint8_t*>(&fake_mode);
  fuchsia_hardware_pwm::PwmConfig fake_config{{
      .polarity = false,
      .period_ns = 1000,
      .duty_cycle = 45.0,
      .mode_config = std::vector<uint8_t>{&fake_mode_bytes[0], &fake_mode_bytes[sizeof(fake_mode)]},
  }};
  EXPECT_OK(pwm()->SetConfig(fake_config));

  fake_mode.mode = 3;
  fake_mode_bytes = reinterpret_cast<uint8_t*>(&fake_mode);
  fake_config.mode_config() =
      std::vector<uint8_t>{&fake_mode_bytes[0], &fake_mode_bytes[sizeof(fake_mode)]};
  fake_config.polarity() = true;
  fake_config.duty_cycle() = 68.0;
  EXPECT_OK(pwm()->SetConfig(fake_config));

  EXPECT_OK(pwm()->SetConfig(fake_config));
}

TEST_F(PwmTest, EnableTest) {
  EXPECT_OK(pwm()->Enable());
  EXPECT_OK(pwm()->Enable());  // Second time
}

TEST_F(PwmTest, DisableTest) {
  EXPECT_OK(pwm()->Disable());
  EXPECT_OK(pwm()->Disable());  // Second time
}

TEST_F(PwmTest, GetConfigFidlTest) {
  // Set a config via the FIDL Pwm interface and validate that the same config is
  // returned via the FIDL PwmImpl interface.
  FakeModeConfig fake_mode{
      .mode = 0xdeadbeef,
  };
  const auto* fake_mode_bytes = reinterpret_cast<uint8_t*>(&fake_mode);
  fuchsia_hardware_pwm::PwmConfig fake_config{{
      .polarity = false,
      .period_ns = 1000,
      .duty_cycle = 45.0,
      .mode_config = std::vector<uint8_t>{&fake_mode_bytes[0], &fake_mode_bytes[sizeof(fake_mode)]},
  }};
  EXPECT_OK(pwm()->SetConfig(fake_config));

  fidl::Result resp = pwm()->GetConfig();

  ASSERT_OK(resp);
  auto& config = resp.value().config();

  WithPwmImpl([](auto& pwm_impl) {
    EXPECT_EQ(pwm_impl.EnableCount(), 0u);
    EXPECT_EQ(pwm_impl.DisableCount(), 0u);
    EXPECT_EQ(pwm_impl.GetConfigCount(), 1u);
    EXPECT_EQ(pwm_impl.SetConfigCount(), 1u);
  });

  EXPECT_EQ(config.polarity(), fake_config.polarity());
  EXPECT_EQ(config.period_ns(), fake_config.period_ns());
  EXPECT_EQ(config.duty_cycle(), fake_config.duty_cycle());
  EXPECT_EQ(config.mode_config(), fake_config.mode_config());
}

TEST_F(PwmTest, SetConfigFidlTest) {
  // Set a config via the FIDL Pwm interface and validate that the same config is
  // returned via the FIDL PwmImpl interface.
  FakeModeConfig fake_mode{
      .mode = 0xdeadbeef,
  };
  const auto* fake_mode_bytes = reinterpret_cast<uint8_t*>(&fake_mode);
  fuchsia_hardware_pwm::PwmConfig config{{
      .polarity = true,
      .period_ns = 1235,
      .duty_cycle = 45.0,
      .mode_config = std::vector<uint8_t>{&fake_mode_bytes[0], &fake_mode_bytes[sizeof(fake_mode)]},
  }};

  EXPECT_OK(pwm()->SetConfig(config));

  fidl::Result fake_config_result = pwm()->GetConfig();
  EXPECT_OK(fake_config_result);
  const auto& fake_config = fake_config_result.value().config();

  WithPwmImpl([](auto& pwm_impl) {
    EXPECT_EQ(pwm_impl.EnableCount(), 0u);
    EXPECT_EQ(pwm_impl.DisableCount(), 0u);
    EXPECT_EQ(pwm_impl.GetConfigCount(), 1u);
    EXPECT_EQ(pwm_impl.SetConfigCount(), 1u);
  });

  EXPECT_EQ(config.polarity(), fake_config.polarity());
  EXPECT_EQ(config.period_ns(), fake_config.period_ns());
  EXPECT_EQ(config.duty_cycle(), fake_config.duty_cycle());
  EXPECT_EQ(config.mode_config(), fake_config.mode_config());
}

TEST_F(PwmTest, EnableFidlTest) {
  ASSERT_OK(pwm()->Enable());

  WithPwmImpl([](auto& pwm_impl) {
    EXPECT_EQ(pwm_impl.EnableCount(), 1u);
    EXPECT_EQ(pwm_impl.DisableCount(), 0u);
    EXPECT_EQ(pwm_impl.GetConfigCount(), 0u);
    EXPECT_EQ(pwm_impl.SetConfigCount(), 0u);
  });
}

TEST_F(PwmTest, DisableFidlTest) {
  ASSERT_OK(pwm()->Disable());

  WithPwmImpl([](auto& pwm_impl) {
    EXPECT_EQ(pwm_impl.EnableCount(), 0u);
    EXPECT_EQ(pwm_impl.DisableCount(), 1u);
    EXPECT_EQ(pwm_impl.GetConfigCount(), 0u);
    EXPECT_EQ(pwm_impl.SetConfigCount(), 0u);
  });
}

class PwmGenericMetadataTest : public ::testing::Test {
 protected:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(PwmGenericMetadataTest, GenericMetadataTest) {
  static const fuchsia_hardware_pwm::PwmChannelsMetadata kTestMetadataChannels{
      {.channels{{{{.id = 0, .period_ns = 1000}}}}}};
  driver_test_.RunInEnvironmentTypeContext(
      [&](PwmTestEnvironment& env) { env.InitGeneric(kTestMetadataChannels); });
  ASSERT_OK(driver_test_.StartDriver());

  zx::result pwm = driver_test_.Connect<fuchsia_hardware_pwm::Service::Pwm>("pwm-0");
  ASSERT_OK(pwm);
}
}  // namespace

}  // namespace pwm
