// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <unistd.h>

#include <array>
#include <filesystem>
#include <vector>

#include <zxtest/zxtest.h>

namespace backlight {

namespace {

namespace FidlBacklight = fuchsia_hardware_backlight;

class BacklightDevice {
 public:
  explicit BacklightDevice(fidl::ClientEnd<FidlBacklight::Device> client_end)
      : client_{std::move(client_end)} {
    if (GetBrightnessNormalized(&orig_brightness_) != ZX_OK) {
      printf("Error getting original brightness. Defaulting to 1.0\n");
      orig_brightness_ = 1.0;
    }
    printf("Brightness at the start of the test: %f\n", orig_brightness_);
  }

  ~BacklightDevice() {
    printf("Restoring original brightness...\n");
    if (SetBrightnessNormalized(orig_brightness_) != ZX_OK) {
      printf("Error setting brightness to %f\n", orig_brightness_);
    }
  }

  zx_status_t GetBrightnessNormalized(double* brightness) {
    auto response = client_->GetStateNormalized();
    zx_status_t status = response.ok() ? (response->is_error() ? response->error_value() : ZX_OK)
                                       : response.status();
    if (status != ZX_OK) {
      return status;
    }
    *brightness = response->value()->state.brightness;
    return status;
  }

  zx_status_t SetBrightnessNormalized(double brightness) {
    FidlBacklight::wire::State state = {.backlight_on = brightness > 0, .brightness = brightness};

    printf("Setting brightness to: %f\n", brightness);
    auto response = client_->SetStateNormalized(state);
    zx_status_t status = response.ok() ? (response->is_error() ? response->error_value() : ZX_OK)
                                       : response.status();
    return status;
  }

  zx_status_t GetBrightnessAbsolute(double* brightness) {
    auto response = client_->GetStateAbsolute();
    zx_status_t status = response.ok() ? (response->is_error() ? response->error_value() : ZX_OK)
                                       : response.status();
    if (status != ZX_OK) {
      return status;
    }
    *brightness = response->value()->state.brightness;
    return status;
  }

  zx_status_t SetBrightnessAbsolute(double brightness) {
    FidlBacklight::wire::State state = {.backlight_on = brightness > 0, .brightness = brightness};

    printf("Setting brightness to: %f nits\n", brightness);
    auto response = client_->SetStateAbsolute(state);
    zx_status_t status = response.ok() ? (response->is_error() ? response->error_value() : ZX_OK)
                                       : response.status();
    return status;
  }

  zx_status_t GetMaxAbsoluteBrightness(double* brightness) {
    auto response = client_->GetMaxAbsoluteBrightness();
    zx_status_t status = response.ok() ? (response->is_error() ? response->error_value() : ZX_OK)
                                       : response.status();
    if (status == ZX_OK) {
      *brightness = response->value()->max_brightness;
    }
    return status;
  }

 private:
  fidl::WireSyncClient<FidlBacklight::Device> client_;
  double orig_brightness_;
};

class BacklightTest : public zxtest::Test {
 public:
  void SetUp() override {
    component::SyncServiceMemberWatcher<FidlBacklight::Service::Backlight> watcher;
    auto result = watcher.GetNextInstance(true);
    while (result.is_ok()) {
      printf("Found backlight device.\n");
      devices_.push_back(std::make_unique<BacklightDevice>(std::move(result.value())));
      result = watcher.GetNextInstance(true);
    }
    if (devices_.empty()) {
      printf("No backlight devices found. Exiting...\n");
    }
  }

  static double Approx(double val) { return round(val * 100.0) / 100.0; }

  static void TestBrightnessNormalized(std::unique_ptr<BacklightDevice>& dev) {
    std::array<double, 9> brightness_values = {0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25, 0.0};

    double brightness;
    for (auto brt : brightness_values) {
      EXPECT_OK(dev->SetBrightnessNormalized(brt));
      EXPECT_OK(dev->GetBrightnessNormalized(&brightness));
      EXPECT_EQ(Approx(brightness), brt);
      SleepIfDelayEnabled();
    }
  }

  static void TestBrightnessAbsolute(std::unique_ptr<BacklightDevice>& dev) {
    double brightness, max_brightness;
    auto status = dev->GetMaxAbsoluteBrightness(&max_brightness);

    if (status == ZX_OK) {
      EXPECT_GT(max_brightness, 0);
      std::array<double, 9> brightness_values = {0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25, 0.0};

      double brightness;
      for (auto brt : brightness_values) {
        EXPECT_OK(dev->SetBrightnessAbsolute(brt * max_brightness));
        EXPECT_OK(dev->GetBrightnessAbsolute(&brightness));
        EXPECT_EQ(Approx(brightness), brt * max_brightness);
        SleepIfDelayEnabled();
      }
    } else {
      EXPECT_EQ(dev->SetBrightnessAbsolute(0), ZX_ERR_NOT_SUPPORTED);
      EXPECT_EQ(dev->GetBrightnessAbsolute(&brightness), ZX_ERR_NOT_SUPPORTED);
    }
  }

  void TestAllDevices() {
    for (auto& dev : devices_) {
      TestBrightnessNormalized(dev);
      TestBrightnessAbsolute(dev);
    }
  }

  static void RunWithDelays() { delayEnabled_ = true; }

  static void SleepIfDelayEnabled() {
    if (delayEnabled_) {
      sleep(1);
    }
  }

 private:
  std::vector<std::unique_ptr<BacklightDevice>> devices_;
  static bool delayEnabled_;
};

bool backlight::BacklightTest::delayEnabled_ = false;

TEST_F(BacklightTest, VaryBrightness) { TestAllDevices(); }

}  // namespace

}  // namespace backlight

int main(int argc, char** argv) {
  int opt;
  while ((opt = getopt(argc, argv, "dh")) != -1) {
    switch (opt) {
      case 'd':
        backlight::BacklightTest::RunWithDelays();
        argc--;
        break;
      case 'h':
      default:
        printf("Usage: runtests --names backlight-test [-- <options>]\n\n");
        printf(
            "  Valid options are:\n"
            "  -d : By default the test runs without any delays between brightness changes.\n"
            "       Pass the -d argument to space the brightness changes one second apart,\n"
            "       so that they are visually perceptible on the screen.\n"
            "  -h : Print this usage text.\n\n");
        return 0;
    }
  }

  return zxtest::RunAllTests(argc, argv);
}
