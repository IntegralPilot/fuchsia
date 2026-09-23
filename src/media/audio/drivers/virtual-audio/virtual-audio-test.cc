// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio/virtual-audio.h"

#include <lib/driver/testing/cpp/driver_test.h>

#include "src/lib/testing/predicates/status.h"
#include "src/media/audio/drivers/virtual-audio/virtual-audio-composite.h"

namespace virtual_audio::test {

namespace {

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override { return zx::ok(); }
};

class TestConfig {
 public:
  using DriverType = VirtualAudio;
  using EnvironmentType = Environment;
};

class VirtualAudioTest : public testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver());
    ConnectToDriver();
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  fidl::SyncClient<fuchsia_virtualaudio::Control>& control_client() { return control_client_; }

 private:
  void ConnectToDriver() {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_virtualaudio::Control>::Create();
    driver_test_.RunInDriverContext(
        [server_end = std::move(server_end)](VirtualAudio& driver) mutable {
          fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server_end),
                           &driver);
        });
    control_client_.Bind(std::move(client_end));
  }

  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fidl::SyncClient<fuchsia_virtualaudio::Control> control_client_;
};

// Verify that the virtual audio driver can start.
TEST_F(VirtualAudioTest, Start) {}

// Verify that the virtual audio driver can provide the default config for virtual audio composite
// devices but not other device types.
TEST_F(VirtualAudioTest, GetDefaultConfig) {
  // Test composite default config.
  {
    fidl::Result result = control_client()->GetDefaultConfiguration({{
        .type = fuchsia_virtualaudio::DeviceType::kComposite,
    }});
    ASSERT_TRUE(result.is_ok());
    auto& default_config = result->config();
    ASSERT_EQ(default_config, VirtualAudioComposite::GetDefaultConfig());
  }

  // Test DAI default config.
  {
    fidl::Result result = control_client()->GetDefaultConfiguration({{
        .type = fuchsia_virtualaudio::DeviceType::kDai,
    }});
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), fuchsia_virtualaudio::Error::kNotSupported);
  }

  // Test stream config default config.
  {
    fidl::Result result = control_client()->GetDefaultConfiguration({{
        .type = fuchsia_virtualaudio::DeviceType::kStreamConfig,
    }});
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), fuchsia_virtualaudio::Error::kNotSupported);
  }

  // Test codec default config.
  {
    fidl::Result result = control_client()->GetDefaultConfiguration({{
        .type = fuchsia_virtualaudio::DeviceType::kDai,
    }});
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), fuchsia_virtualaudio::Error::kNotSupported);
  }
}

}  // namespace

}  // namespace virtual_audio::test
