// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/adc/drivers/adc/adc.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.adcimpl/cpp/driver/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <algorithm>
#include <cmath>

#include <gtest/gtest.h>
#include <sdk/lib/driver/metadata/cpp/metadata_server.h>

#include "src/devices/lib/fidl-metadata/adc.h"

namespace adc {

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

class FakeAdcImplServer : public fdf::Server<fuchsia_hardware_adcimpl::Device> {
 public:
  ~FakeAdcImplServer() {
    for (const auto& [_, expected] : expected_samples_) {
      EXPECT_TRUE(expected.empty());
    }
  }

  void set_resolution(uint8_t resolution) { resolution_ = resolution; }
  void ExpectGetSample(uint32_t channel, uint32_t sample) {
    expected_samples_[channel].push(sample);
  }

  void GetResolution(GetResolutionCompleter::Sync& completer) override {
    completer.Reply(fit::ok(resolution_));
  }
  void GetSample(GetSampleRequest& request, GetSampleCompleter::Sync& completer) override {
    ASSERT_FALSE(expected_samples_.empty());
    ASSERT_NE(expected_samples_.find(request.channel_id()), expected_samples_.end());
    ASSERT_FALSE(expected_samples_.at(request.channel_id()).empty());
    completer.Reply(fit::ok(expected_samples_.at(request.channel_id()).front()));
    expected_samples_.at(request.channel_id()).pop();
  }

  fuchsia_hardware_adcimpl::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_adcimpl::Service::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

 private:
  uint8_t resolution_ = 0;
  std::map<uint32_t, std::queue<uint32_t>> expected_samples_;

  fdf::ServerBindingGroup<fuchsia_hardware_adcimpl::Device> bindings_;
};

class AdcTestEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    zx::result result = to_driver_vfs.AddService<fuchsia_hardware_adcimpl::Service>(
        fake_adc_impl_server_.GetInstanceHandler());
    if (result.is_error()) {
      return result.take_error();
    }

    if (metadata_.has_value()) {
      if (zx::result result = metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(), metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (generic_metadata_.has_value()) {
      if (zx::result result = generic_metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              "fuchsia.hardware.adcimpl.Metadata", generic_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  void Init(std::vector<fidl_metadata::adc::Channel> kAdcChannels) {
    std::vector<fuchsia_hardware_adcimpl::AdcChannel> channels;
    std::ranges::transform(kAdcChannels, std::back_inserter(channels), [](const auto& channel) {
      return fuchsia_hardware_adcimpl::AdcChannel{{.idx = channel.idx, .name = channel.name}};
    });
    metadata_ = fuchsia_hardware_adcimpl::Metadata({.channels = std::move(channels)});
  }

  void InitGeneric(const std::vector<fidl_metadata::adc::Channel>& kAdcChannels) {
    std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
    entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
        "channels._count", fuchsia_driver_metadata::DictionaryValue::WithInt64(
                               static_cast<int64_t>(kAdcChannels.size()))));
    for (size_t i = 0; i < kAdcChannels.size(); ++i) {
      std::string base_key = "channels." + std::to_string(i) + ".";
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          base_key + "channel", fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                    static_cast<int64_t>(kAdcChannels[i].idx))));
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          base_key + "name",
          fuchsia_driver_metadata::DictionaryValue::WithStr(std::string(kAdcChannels[i].name))));
    }
    generic_metadata_ = fuchsia_driver_metadata::Dictionary({.entries = std::move(entries)});
  }

  FakeAdcImplServer& fake_adc_impl_server() { return fake_adc_impl_server_; }

 private:
  FakeAdcImplServer fake_adc_impl_server_;
  fdf_metadata::MetadataServer<fuchsia_hardware_adcimpl::Metadata> metadata_server_;
  std::optional<fuchsia_hardware_adcimpl::Metadata> metadata_;
  GenericMetadataServer generic_metadata_server_;
  std::optional<fuchsia_driver_metadata::Dictionary> generic_metadata_;
};

class AdcTestConfig final {
 public:
  using DriverType = adc::Adc;
  using EnvironmentType = AdcTestEnvironment;
};

class AdcTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  zx::result<> Init(const std::vector<fidl_metadata::adc::Channel>& kAdcChannels) {
    driver_test().RunInEnvironmentTypeContext(
        [kAdcChannels](AdcTestEnvironment& env) { env.Init(kAdcChannels); });
    return driver_test().StartDriver();
  }

  zx::result<> InitGeneric(const std::vector<fidl_metadata::adc::Channel>& kAdcChannels) {
    driver_test().RunInEnvironmentTypeContext(
        [kAdcChannels](AdcTestEnvironment& env) { env.InitGeneric(kAdcChannels); });
    return driver_test().StartDriver();
  }
  fidl::ClientEnd<fuchsia_hardware_adc::Device> GetClient(uint32_t channel) {
    // Connect to Adc.
    auto result =
        driver_test().Connect<fuchsia_hardware_adc::Service::Device>(std::to_string(channel));
    EXPECT_EQ(ZX_OK, result.status_value());
    return std::move(result.value());
  }

  fdf_testing::BackgroundDriverTest<AdcTestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<AdcTestConfig> driver_test_;
};

TEST_F(AdcTest, CreateDevicesTest) {
  auto result = Init({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4)});
  ASSERT_TRUE(result.is_ok());

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(node.children().size(), 2ul);
    EXPECT_NE(node.children().find("1"), node.children().end());
    EXPECT_NE(node.children().find("4"), node.children().end());
  });
}

TEST_F(AdcTest, OverlappingChannelsTest) {
  auto result = Init({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4), DECL_ADC_CHANNEL(1)});
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.error_value(), ZX_ERR_INVALID_ARGS);
}

TEST_F(AdcTest, GetResolutionTest) {
  driver_test().RunInEnvironmentTypeContext(
      [](AdcTestEnvironment& env) { env.fake_adc_impl_server().set_resolution(12); });
  auto result = Init({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4)});
  ASSERT_TRUE(result.is_ok());

  auto resolution = fidl::WireCall(GetClient(1))->GetResolution();
  ASSERT_TRUE(resolution.ok());
  ASSERT_TRUE(resolution->is_ok());
  EXPECT_EQ(resolution.value()->resolution, 12);
}

TEST_F(AdcTest, GetSampleTest) {
  auto result = Init({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4)});
  ASSERT_TRUE(result.is_ok());

  driver_test().RunInEnvironmentTypeContext(
      [](AdcTestEnvironment& env) { env.fake_adc_impl_server().ExpectGetSample(1, 20); });
  auto sample = fidl::WireCall(GetClient(1))->GetSample();
  ASSERT_TRUE(sample.ok());
  ASSERT_TRUE(sample->is_ok());
  EXPECT_EQ(sample.value()->value, 20u);
}

TEST_F(AdcTest, GetNormalizedSampleTest) {
  driver_test().RunInEnvironmentTypeContext([](AdcTestEnvironment& env) {
    env.fake_adc_impl_server().set_resolution(2);
    env.fake_adc_impl_server().ExpectGetSample(4, 9);
  });

  auto result = Init({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4)});
  ASSERT_TRUE(result.is_ok());

  auto sample = fidl::WireCall(GetClient(4))->GetNormalizedSample();
  ASSERT_TRUE(sample.ok());
  ASSERT_TRUE(sample->is_ok());
  EXPECT_EQ(std::lround(sample.value()->value), 3);
}

TEST_F(AdcTest, ChannelOutOfBoundsTest) {
  auto result = Init({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4)});
  ASSERT_TRUE(result.is_ok());

  driver_test().RunInEnvironmentTypeContext(
      [](AdcTestEnvironment& env) { env.fake_adc_impl_server().set_resolution(12); });
  auto resolution = fidl::WireCall(GetClient(3))->GetResolution();
  ASSERT_FALSE(resolution.ok());
}

TEST_F(AdcTest, GenericMetadataTest) {
  auto result = InitGeneric({DECL_ADC_CHANNEL(1), DECL_ADC_CHANNEL(4)});
  ASSERT_TRUE(result.is_ok());

  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(node.children().size(), 2ul);
    EXPECT_NE(node.children().find("1"), node.children().end());
    EXPECT_NE(node.children().find("4"), node.children().end());
  });
}

}  // namespace adc
