// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_TESTS_DEVICE_HOST_H_
#define SRC_MEDIA_AUDIO_DRIVERS_TESTS_DEVICE_HOST_H_

#include <fuchsia/virtualaudio/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>

#include <atomic>
#include <optional>

#include "src/lib/fsl/io/device_watcher.h"
#include "src/media/audio/drivers/tests/test_base.h"

namespace media::audio::drivers::test {

class DeviceHost {
 public:
  DeviceHost();
  ~DeviceHost();

  // Post a task to our thread to detect and add all devices, so that driver testing can begin.
  void AddDevices(bool no_bluetooth, bool no_virtual_audio);

  // Create testcase instances for each device entry, based on the passed-in configuration.
  void RegisterTests(bool enable_basic_tests, bool enable_admin_tests, bool enable_position_tests);

  // Testing is complete. Clean up our virtual audio devices and shut down our loop.
  zx_status_t QuitDeviceLoop();

 private:
  // Detect devfs-based devices, optionally adding device entries for bluetooth and virtual devices.
  void DetectDevices(bool no_bluetooth, bool no_virtual_audio);

  // Optionally called during DetectDevices. Create virtual_audio instances for each device type
  // using the default configurations settings (which should always pass all tests).
  void AddVirtualDevices();
  void RegisterVirtualAudioDrivers();
  void UnregisterVirtualAudioDrivers();
  void WaitForDeviceNode(const std::string& directory_path, const std::string& expected_filename);
  void AddVirtualDevice(fuchsia::virtualaudio::ControlSyncPtr& controller,
                        fuchsia::virtualaudio::DeviceType device_type,
                        std::optional<bool> is_input = std::nullopt);

  std::set<DeviceEntry>& device_entries() { return device_entries_; }
  std::vector<std::unique_ptr<fsl::DeviceWatcher>>& device_watchers() { return device_watchers_; }

  async::Loop device_loop_;
  std::set<DeviceEntry> device_entries_;
  std::vector<std::unique_ptr<fsl::DeviceWatcher>> device_watchers_;

  // While the test suite is running, we spawn a number of virtual_audio driver instances.
  static constexpr size_t kNumVirtualAudioDevicesToAdd = 5;
  fuchsia::virtualaudio::ControlSyncPtr controller_ = nullptr;
  fuchsia::virtualaudio::ControlSyncPtr legacy_controller_ = nullptr;
  std::vector<fuchsia::virtualaudio::DevicePtr> virtual_audio_devices_;

  bool shutting_down_ = false;
  bool no_virtual_audio_ = false;
  std::vector<std::string> registered_driver_urls_;
  std::vector<std::string> added_test_nodes_;
  std::atomic_bool device_enumeration_complete_ = false;
};

}  // namespace media::audio::drivers::test

#endif  // SRC_MEDIA_AUDIO_DRIVERS_TESTS_DEVICE_HOST_H_
