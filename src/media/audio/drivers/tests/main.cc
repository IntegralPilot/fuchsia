// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/log_settings.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/test/test_settings.h"
#include "src/media/audio/drivers/tests/device_host.h"

namespace {

inline constexpr std::string kHelpOption = "help";
inline constexpr std::string kHelpOption2 = "?";
inline constexpr std::string kNoBluetoothOption = "no-bluetooth";
inline constexpr std::string kNoVirtualAudioOption = "no-virtual";
inline constexpr std::string kRunBasicTestsOption = "basic";
inline constexpr std::string kRunAdminTestsOption = "admin";
inline constexpr std::string kRunPositionTestsOption = "position";
inline constexpr std::string kRunAllTestsOption = "all";

void usage(const std::string& prog_name) {
  printf("\n");
  printf("Usage: %s -- [--option] [--option] [...]\n\n", prog_name.c_str());

  printf("Test audio drivers via direct FIDL calls\n\n");

  printf("This test suite validates audio drivers. By default, it tests those that it finds in\n");
  printf("devfs, all virtual_audio device types, and the helper library used by Bluetooth to\n");
  printf("implement audio drivers.\n\n");

  printf("By default, this suite assumes that an audio service is already running and connected\n");
  printf("to audio drivers. For this reason, for devfs drivers it does not execute test cases\n");
  printf("that require exclusive (or 'admin') access -- unless explicitly directed to do so.\n\n");

  printf("An additional set of privileged tests, not executed automatically with the other\n");
  printf("'admin' test cases, is the 'position' tests. The user should only execute these cases\n");
  printf("on a system with guaranteed real-time response, such as a non-emulated environment\n");
  printf("running a release build with standard optimizations.\n\n");

  printf("Valid options:\n\n");

  printf("  --%s\t   Do not test the Bluetooth helper library; only test drivers found in devfs\n",
         kNoBluetoothOption.c_str());
  printf("  --%s\t\t   Do not instantiate+test 'virtual_audio' instances\n",
         kNoVirtualAudioOption.c_str());
  printf("  \t\t\t   Note: this switch allows any ALREADY-RUNNING 'virtual_audio' instances to\n");
  printf("  \t\t\t   be detected in devfs and tested, like drivers for real physical devices\n");
  printf("  --%s\t\t   Run tests that can execute while an audio service (AudioCore or \n",
         kRunBasicTestsOption.c_str());
  printf("  \t\t\t    AudioDeviceRegistry) is running.\n");
  printf("  --%s\t\t   Run admin tests (requires exclusive access - no audio service running)\n",
         kRunAdminTestsOption.c_str());
  printf("  --%s\t\t   Run position tests (assumes 'release' with standard optimizations)\n",
         kRunPositionTestsOption.c_str());
  printf("  --%s, --%s\t\t   Show this message\n\n", kHelpOption.c_str(), kHelpOption2.c_str());
}

}  // namespace

int main(int argc, char** argv) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (!fxl::SetTestSettings(command_line)) {
    return EXIT_FAILURE;
  }

  if (command_line.HasOption(kHelpOption) || command_line.HasOption(kHelpOption2)) {
    std::string app_name, sub_str;
    std::stringstream ss(command_line.argv0());
    while (std::getline(ss, sub_str, '/')) {
      app_name = sub_str;
    }
    usage(app_name);
    return EXIT_SUCCESS;
  }

  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({command_line.argv0()}).BuildAndInitialize();
  testing::InitGoogleTest(&argc, argv);

  // The default configuration for all six of the cmdline options is FALSE.

  // Options that filter which drivers are validated.
  //
  // --no-bluetooth: Only detect+test devices in devfs; don't add+test the Bluetooth audio library.
  bool no_bluetooth = command_line.HasOption(kNoBluetoothOption);
  //
  // --no-virtual: Don't automatically create+test virtual_audio instances for StreamConfig, Dai,
  //   Codec and Composite (using default settings). When this flag is enabled, any _preexisting_
  //   virtual_audio instances are allowed and detected+tested like any other devfs device.
  bool no_virtual_audio = command_line.HasOption(kNoVirtualAudioOption);

  // Options that filter which tests are executed.
  //
  // --basic: Validate commands that do not require exclusive access, such as SetFormat.
  bool enable_basic_tests = command_line.HasOption(kRunBasicTestsOption);
  //
  // --admin: Run tests that require exclusive access to the driver (AdminTest cases).
  //   TODO(https://fxbug.dev/42175212): Auto-detect whether a service is already connected to the
  //   driver, and eliminate the 'basic ' and 'admin' flags.
  bool enable_admin_tests = command_line.HasOption(kRunAdminTestsOption);
  //
  // --position: Include audio position test cases (requires realtime capable system).
  bool enable_position_tests = command_line.HasOption(kRunPositionTestsOption);
  //
  // --all: basic+admin+position (currently used only for testing on eng desktop.
  if (command_line.HasOption(kRunAllTestsOption)) {
    enable_basic_tests = enable_admin_tests = enable_position_tests = true;
  }

  media::audio::drivers::test::DeviceHost device_host;
  device_host.AddDevices(no_bluetooth, no_virtual_audio);
  device_host.RegisterTests(enable_basic_tests, enable_admin_tests, enable_position_tests);

  auto ret = RUN_ALL_TESTS();

  if (device_host.QuitDeviceLoop() != ZX_OK) {
    FX_LOGS(ERROR) << "DeviceHost::QuitDeviceLoop timed out";
    return EXIT_FAILURE;
  }

  return ret;
}
