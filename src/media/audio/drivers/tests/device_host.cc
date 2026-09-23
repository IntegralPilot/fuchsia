// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/tests/device_host.h"

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.driver.registrar/cpp/fidl.h>
#include <fuchsia/virtualaudio/cpp/fidl.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/system/public/zircon/compiler.h>

#include <filesystem>
#include <format>
#include <string>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

#include "src/lib/fsl/io/device_watcher.h"
#include "src/media/audio/drivers/tests/test_base.h"

namespace media::audio::drivers::test {

// TODO(https://fxbug.dev/42144297): Previous implementation used value-parameterized testing.
// Consider reverting to this, moving AddDevices to a function called at static initialization time.
// If we cannot access cmdline flags at that time, this would force us to always register admin
// tests, skipping them at runtime based on the cmdline flag.

extern void RegisterBasicTestsForDevice(const DeviceEntry& device_entry);
extern void RegisterAdminTestsForDevice(const DeviceEntry& device_entry);
extern void RegisterPositionTestsForDevice(const DeviceEntry& device_entry);

static const struct {
  const char* path;
  const char* member;
  DriverType driver_type;
} kAudioServices[] = {
    {.path = "/svc/fuchsia.hardware.audio.CompositeConnectorService",
     .member = "composite_connector",
     .driver_type = DriverType::Composite},
    {.path = "/svc/fuchsia.hardware.audio.StreamConfigConnectorInputService",
     .member = "stream_config_connector",
     .driver_type = DriverType::StreamConfigInput},
    {.path = "/svc/fuchsia.hardware.audio.StreamConfigConnectorOutputService",
     .member = "stream_config_connector",
     .driver_type = DriverType::StreamConfigOutput},
    {.path = "/svc/fuchsia.hardware.audio.CodecConnectorService",
     .member = "codec_connector",
     .driver_type = DriverType::Codec},
    {.path = "/svc/fuchsia.hardware.audio.DaiConnectorService",
     .member = "dai_connector",
     .driver_type = DriverType::Dai},
};

// Our thread and dispatcher must exist during the entirety of test execution; create it now.
DeviceHost::DeviceHost() : device_loop_(async::Loop(&kAsyncLoopConfigNeverAttachToThread)) {
  device_loop_.StartThread("AddVadAndDetectDevices");
}
DeviceHost::~DeviceHost() { QuitDeviceLoop(); }

// Post a task to our thread to detect and add all devices, so that testing can begin.
void DeviceHost::AddDevices(bool no_bluetooth, bool no_virtual_audio) {
  libsync::Completion done;
  async::PostTask(device_loop_.dispatcher(), [this, &done, no_bluetooth, no_virtual_audio]() {
    DetectDevices(no_bluetooth, no_virtual_audio);
    done.Signal();
  });
  // If we hang indefinitely here, the test execution environment will eventually timeout.
  done.Wait();
}

// Set up DeviceWatchers to detect audio devices.
//
// First, detect audio devices that were already in devfs when we started the detection process.
//
// Following this, (optionally) add any virtual_audio devices and rely on our previously-installed
// device watchers to detect them. (NOTE: subsequent device arrivals/departures that happen outside
// the control of this suite are treated as immediate failures.)
//
// We then (optionally) add an instance of the Bluetooth audio device library.
//
// Note that with the current design, we must keep the DeviceWatchers alive so that each device's
// fuchsia_io::Directory is not dropped. This means that the DeviceWatcher callback might run after
// this method exits. This requires us to use class member `device_enumeration_complete_` to signal
// that these subsequent device-detection callbacks should trigger immediate failures instead of
// treating this like another device to be tested.
void DeviceHost::DetectDevices(bool no_bluetooth, bool no_virtual_audio) {
  no_virtual_audio_ = no_virtual_audio;
  // This is guarded by `device_enumeration_complete_` which we set before we exit, but we give this
  // variable static scope to avoid future issues.
  static DeviceType dev_type = DeviceType::BuiltIn;

  // Ensure that an initial devfs enumeration pass completes before creating the next watcher.
  // This is only accessed by the idle_callback, which we explicitly await before we exit. Just in
  // case the callback can subsequently run for some reason, we give this variable static scope.
  static volatile bool initial_enumeration_done;

  // Set up the device watchers. If any fail, automatically stop monitoring all device sources.
  // First, we add any preexisting ("built-in") devices.
  for (const auto& service : kAudioServices) {
    initial_enumeration_done = false;
    auto watcher = fsl::DeviceWatcher::CreateWithIdleCallback(
        service.path,
        [this, driver_type = service.driver_type, member = service.member](
            const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& filename) {
          ASSERT_FALSE(device_enumeration_complete_)
              << "Unexpected audio device detection occurred after test suite configuration";

          FX_LOGS(TRACE) << "dir handle " << dir.channel().get() << " for '" << filename << "' ("
                         << dev_type << " " << driver_type << ")";
          device_entries().insert({.dir = dir,
                                   .filename = filename + "/" + member,
                                   .driver_type = driver_type,
                                   .device_type = dev_type});
        },
        []() { initial_enumeration_done = true; }, device_loop_.dispatcher());

    if (watcher == nullptr) {
      ASSERT_FALSE(watcher == nullptr)
          << "AudioDriver::TestBase failed creating DeviceWatcher for '" << service.path << "'.";
    }

    // If we hang indefinitely here, the test execution environment will eventually timeout.
    while (!initial_enumeration_done) {
      device_loop_.RunUntilIdle();
    }
    ASSERT_TRUE(initial_enumeration_done)
        << "DeviceWatcher did not finish initial enumeration, for " << dev_type << "/"
        << service.driver_type;

    // We must save this so each device's fidl::ClientEnd<fuchsia_io::Directory is not dropped.
    device_watchers().emplace_back(std::move(watcher));
  }

  // Then, if enabled, enable virtual_audio instances and wait for their detection.
  // By reusing the watchers we've already configured, we detect each preexisting device only once.
  if (!no_virtual_audio) {
    auto real_device_count = device_entries().size();
    dev_type = DeviceType::Virtual;
    ASSERT_NO_FAILURE_OR_SKIP(AddVirtualDevices());

    // If we hang indefinitely here, the test execution environment will eventually timeout.
    auto device_count = real_device_count + virtual_audio_devices_.size();
    while (device_entries().size() < device_count) {
      device_loop_.RunUntilIdle();
    }
    ASSERT_GE(device_entries().size(), device_count)
        << "DeviceWatcher timed out, for " << dev_type << " devices";
  }

  // If any subsequent device detections occur, we consider these errors.
  device_enumeration_complete_ = true;

  // And finally, unless expressly excluded, manually add a device entry for the Bluetooth audio
  // library, to validate admin functions even if AudioCore has connected to "real" audio drivers.
  if (!no_bluetooth) {
    device_entries().insert({
        .dir = {},
        .filename = "A2DP",
        .driver_type = DriverType::StreamConfigOutput,
        .device_type = DeviceType::A2DP,
    });
  }
}

// Optionally called during DetectDevices. Create virtual_audio instances (all four types) using the
// default configuration settings (which should pass all tests).
void DeviceHost::AddVirtualDevices() {
  RegisterVirtualAudioDrivers();

  // Add virtual audio devices using non-legacy controller.
  {
    std::string parent_dir = std::filesystem::exists("/dev/sys/platform/virtual-audio")
                                 ? "/dev/sys/platform/virtual-audio"
                                 : "/dev/topological/virtual-audio";
    WaitForDeviceNode(parent_dir, "virtual-audio");
    const std::string kControlNodePath = parent_dir + "/virtual-audio";
    zx_status_t status = fdio_service_connect(kControlNodePath.c_str(),
                                              controller_.NewRequest().TakeChannel().release());
    ASSERT_EQ(status, ZX_OK) << "fdio_service_connect(" << kControlNodePath
                             << ") failed: " << status;

    uint32_t num_inputs = -1, num_outputs = -1, num_unspecified_direction = -1;
    status = controller_->GetNumDevices(&num_inputs, &num_outputs, &num_unspecified_direction);
    ASSERT_EQ(status, ZX_OK) << "GetNumDevices failed";
    ASSERT_TRUE(controller_.is_bound()) << "virtualaudio::Control did not stay bound";
    ASSERT_EQ(num_inputs, 0u) << num_inputs << " virtual-audio inputs already exist (should be 0)";
    ASSERT_EQ(num_outputs, 0u) << num_outputs
                               << " virtual-audio outputs already exist (should be 0)";
    ASSERT_EQ(num_unspecified_direction, 0u)
        << num_unspecified_direction
        << " virtual-audio devices with unspecified direction already exist (should be 0)";

    // Composite has no directionality; for this testing.
    AddVirtualDevice(controller_, fuchsia::virtualaudio::DeviceType::COMPOSITE);
    // This step might have caused a test case failure, so for subsequent steps we use
    // ASSERT_NO_FAILURE_OR_SKIP in order to fast-fail.
  }

  // Add virtual audio devices using legacy controller.
  {
    std::string legacy_parent_dir =
        std::filesystem::exists("/dev/sys/platform/virtual-audio-legacy")
            ? "/dev/sys/platform/virtual-audio-legacy"
            : "/dev/topological/virtual-audio-legacy";
    WaitForDeviceNode(legacy_parent_dir, "virtual-audio-legacy");
    const std::string kLegacyControlNodePath = legacy_parent_dir + "/virtual-audio-legacy";
    zx_status_t status = fdio_service_connect(
        kLegacyControlNodePath.c_str(), legacy_controller_.NewRequest().TakeChannel().release());
    ASSERT_EQ(status, ZX_OK) << "fdio_service_connect(" << kLegacyControlNodePath
                             << ") failed: " << status;

    uint32_t num_inputs = -1, num_outputs = -1, num_unspecified_direction = -1;
    status =
        legacy_controller_->GetNumDevices(&num_inputs, &num_outputs, &num_unspecified_direction);
    ASSERT_EQ(status, ZX_OK) << "GetNumDevices(legacy) failed: " << status;
    ASSERT_TRUE(legacy_controller_.is_bound())
        << "virtualaudio::Control(legacy) did not stay bound";
    ASSERT_EQ(num_inputs, 0u)
        << num_inputs << " virtual-audio-legacy 'input' devices already exist (should be 0)";
    ASSERT_EQ(num_outputs, 0u)
        << num_outputs << " virtual-audio-legacy 'output' devices already exist (should be 0)";
    ASSERT_EQ(num_unspecified_direction, 0u)
        << num_unspecified_direction
        << " virtual-audio-legacy 'unspecified direction' devices already exist (should be 0)";

    // For Codec drivers, directionality is not applicable.
    ASSERT_NO_FAILURE_OR_SKIP(
        AddVirtualDevice(legacy_controller_, fuchsia::virtualaudio::DeviceType::CODEC));

    ASSERT_NO_FAILURE_OR_SKIP(
        AddVirtualDevice(legacy_controller_, fuchsia::virtualaudio::DeviceType::DAI, true));
    ASSERT_NO_FAILURE_OR_SKIP(
        AddVirtualDevice(legacy_controller_, fuchsia::virtualaudio::DeviceType::DAI, false));
    ASSERT_NO_FAILURE_OR_SKIP(AddVirtualDevice(
        legacy_controller_, fuchsia::virtualaudio::DeviceType::STREAM_CONFIG, true));
    ASSERT_NO_FAILURE_OR_SKIP(AddVirtualDevice(
        legacy_controller_, fuchsia::virtualaudio::DeviceType::STREAM_CONFIG, false));
  }
}

// If the base OS already provides virtual-audio platform nodes (e.g. workbench, smart_display),
// reuse them directly. Else (e.g. minimal, legacy products without the modern virtual-audio),
// dynamically create topological test nodes and register ephemeral drivers packaged with the test.
void DeviceHost::RegisterVirtualAudioDrivers() {
  if (no_virtual_audio_) {
    return;
  }

  auto dev_mgr = component::Connect<fuchsia_driver_development::Manager>();
  std::optional<fidl::SyncClient<fuchsia_driver_development::Manager>> mgr_client;
  if (dev_mgr.is_ok()) {
    mgr_client.emplace(std::move(*dev_mgr));
  } else {
    FX_LOGS(WARNING) << "Could not connect to fuchsia.driver.development.Manager: "
                     << dev_mgr.status_string();
  }

  bool has_modern_driver = false;
  bool has_legacy_driver = false;

  if (mgr_client.has_value()) {
    auto& mgr = mgr_client.value();
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_driver_development::DriverInfoIterator>::Create();
    if (auto res = mgr->GetDriverInfo({{
            .driver_filter = {},
            .iterator = std::move(server_end),
        }});
        res.is_ok()) {
      fidl::SyncClient iterator(std::move(client_end));
      while (true) {
        auto next = iterator->GetNext();
        if (next.is_error() || next->drivers().empty()) {
          break;
        }
        for (const auto& driver : next->drivers()) {
          if (driver.url().has_value() && driver.package_type().has_value()) {
            bool is_base_or_boot =
                (*driver.package_type() == fuchsia_driver_framework::DriverPackageType::kBoot ||
                 *driver.package_type() == fuchsia_driver_framework::DriverPackageType::kBase);
            if (is_base_or_boot) {
              const std::string& url = *driver.url();
              if (url.starts_with("fuchsia-pkg://fuchsia.com/virtual-audio#") ||
                  url.starts_with("fuchsia-boot:///virtual-audio#")) {
                has_modern_driver = true;
              }
              if (url.starts_with("fuchsia-pkg://fuchsia.com/virtual-audio-legacy#") ||
                  url.starts_with("fuchsia-boot:///virtual-audio-legacy#")) {
                has_legacy_driver = true;
              }
            }
          }
        }
      }
    }
  }

  bool need_modern_driver = !has_modern_driver;
  bool need_legacy_driver = !has_legacy_driver;
  if (!need_modern_driver && !need_legacy_driver) {
    FX_LOGS(INFO) << "Base virtual audio drivers already present; skipping ephemeral registration.";
    return;
  }

  auto registrar = component::Connect<fuchsia_driver_registrar::DriverRegistrar>();
  if (registrar.is_error()) {
    FX_LOGS(WARNING) << "Could not connect to fuchsia.driver.registrar.DriverRegistrar: "
                     << registrar.status_string();
    return;
  }
  fidl::SyncClient client(std::move(*registrar));

  std::string current_pkg_name;
  std::vector<std::string> package_names = {
      "audio_driver_basic_tests",
      "audio_driver_admin_tests",
      "audio_driver_realtime_tests",
  };
  for (const auto& pkg_name : package_names) {
    if (std::filesystem::exists("/pkg/meta/" + pkg_name + ".cm")) {
      current_pkg_name = pkg_name;
      break;
    }
  }
  if (current_pkg_name.empty()) {
    FX_LOGS(WARNING)
        << "Could not determine pkg from /pkg/meta/, falling back to audio_driver_basic_tests";
    current_pkg_name = "audio_driver_basic_tests";
  }

  // Disable ephemeral drivers left behind by other test packages (e.g. other ADT suites: admin,
  // basic, realtime) so driver_index does not encounter conflicting matches for the same node.
  auto restart_flags = fuchsia_driver_development::RestartRematchFlags::kRequested |
                       fuchsia_driver_development::RestartRematchFlags::kCompositeSpec;

  if (mgr_client.has_value()) {
    auto& mgr = mgr_client.value();
    for (const auto& pkg_name : package_names) {
      if (pkg_name == current_pkg_name) {
        continue;
      }
      for (const auto& driver_cm : {"virtual-audio-driver.cm", "virtual-audio-legacy-driver.cm"}) {
        std::string url = std::format("fuchsia-pkg://fuchsia.com/{}#meta/{}", pkg_name, driver_cm);
        if (auto disable_res = mgr->DisableDriver({{.driver_url = url}}); disable_res.is_error()) {
          FX_LOGS(WARNING) << "DisableDriver(" << url
                           << ") returned: " << disable_res.error_value().FormatDescription();
        }
        if (auto restart_res = mgr->RestartDriverHosts({{
                .driver_url = url,
                .rematch_flags = restart_flags,
            }});
            restart_res.is_error()) {
          FX_LOGS(WARNING) << "RestartDriverHosts(" << url
                           << ") returned: " << restart_res.error_value().FormatDescription();
        }
      }
    }

    struct TestNodeSpec {
      std::string name;
      std::string compatible;
    };
    std::vector<TestNodeSpec> specs_to_add;

    if (need_modern_driver) {
      specs_to_add.push_back(TestNodeSpec{
          .name = "virtual-audio",
          .compatible = "fuchsia,virtual-audio",
      });
    }
    if (need_legacy_driver) {
      specs_to_add.push_back(TestNodeSpec{
          .name = "virtual-audio-legacy",
          .compatible = "fuchsia,virtual-audio-legacy",
      });
    }

    for (const auto& spec : specs_to_add) {
      fuchsia_driver_development::TestNodeAddArgs args;
      args.name(spec.name);
      args.properties(std::vector<fuchsia_driver_framework::NodeProperty>{
          fuchsia_driver_framework::NodeProperty{{
              fuchsia_driver_framework::NodePropertyKey::WithStringValue(bind_fuchsia::COMPATIBLE),
              fuchsia_driver_framework::NodePropertyValue::WithStringValue(spec.compatible),
          }},
      });
      auto add_res = mgr->AddTestNode({{.args = std::move(args)}});
      if (add_res.is_ok() || (add_res.is_error() && add_res.error_value().is_domain_error() &&
                              add_res.error_value().domain_error() ==
                                  fuchsia_driver_framework::NodeError::kNameAlreadyExists)) {
        FX_LOGS(INFO) << "Added or verified existing test node " << spec.name;
        added_test_nodes_.push_back(spec.name);
      } else {
        FX_LOGS(INFO) << "AddTestNode(" << spec.name
                      << ") returned error: " << add_res.error_value().FormatDescription();
      }
    }
  }

  // Now register and enable the needed driver manifests from the current package.
  std::vector<const char*> drivers_to_register;
  if (need_modern_driver) {
    drivers_to_register.push_back("virtual-audio-driver.cm");
  }
  if (need_legacy_driver) {
    drivers_to_register.push_back("virtual-audio-legacy-driver.cm");
  }
  for (const auto& driver_cm : drivers_to_register) {
    std::string url =
        std::format("fuchsia-pkg://fuchsia.com/{}#meta/{}", current_pkg_name, driver_cm);
    if (mgr_client.has_value()) {
      auto& mgr = mgr_client.value();
      if (auto enable_res = mgr->EnableDriver({{.driver_url = url}}); enable_res.is_error()) {
        FX_LOGS(WARNING) << "EnableDriver(" << url
                         << ") returned: " << enable_res.error_value().FormatDescription();
      }
    }
    auto result = client->Register({url});
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Registering " << url
                       << " returned error: " << result.error_value().FormatDescription();
    } else {
      FX_LOGS(INFO) << "Registered ephemeral driver " << url;
      registered_driver_urls_.push_back(url);
    }
  }

  if (mgr_client.has_value()) {
    auto& mgr = mgr_client.value();
    auto bind_result = mgr->BindAllUnboundNodes2();
    if (bind_result.is_ok()) {
      FX_LOGS(INFO) << "BindAllUnboundNodes2 succeeded, bound "
                    << bind_result->binding_result().size() << " nodes.";
    } else {
      FX_LOGS(WARNING) << "BindAllUnboundNodes2 error: "
                       << bind_result.error_value().FormatDescription();
    }
  }
}

// Teardown any ephemeral test nodes and ephemeral drivers registered by this test run.
// Base system drivers are left untouched.
void DeviceHost::UnregisterVirtualAudioDrivers() {
  if (no_virtual_audio_ || (added_test_nodes_.empty() && registered_driver_urls_.empty())) {
    return;
  }
  auto dev_mgr = component::Connect<fuchsia_driver_development::Manager>();
  if (dev_mgr.is_error()) {
    FX_LOGS(WARNING)
        << "Could not connect to fuchsia.driver.development.Manager for unregistering: "
        << dev_mgr.status_string();
    return;
  }
  fidl::SyncClient mgr_client(std::move(*dev_mgr));

  auto restart_flags = fuchsia_driver_development::RestartRematchFlags::kRequested |
                       fuchsia_driver_development::RestartRematchFlags::kCompositeSpec;

  for (const auto& url : registered_driver_urls_) {
    auto result = mgr_client->DisableDriver({{.driver_url = url}});
    auto restart_result = mgr_client->RestartDriverHosts({{
        .driver_url = url,
        .rematch_flags = restart_flags,
    }});
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Unregistering (DisableDriver) " << url
                       << " returned error: " << result.error_value().FormatDescription();
    } else {
      FX_LOGS(INFO) << "Unregistered ephemeral driver " << url;
    }
    if (restart_result.is_error()) {
      FX_LOGS(WARNING) << "Restarting driver host for " << url
                       << " returned error: " << restart_result.error_value().FormatDescription();
    }
  }
  registered_driver_urls_.clear();

  for (const auto& node_name : added_test_nodes_) {
    auto remove_res = mgr_client->RemoveTestNode({{.name = node_name}});
    if (remove_res.is_ok()) {
      FX_LOGS(INFO) << "Removed test node " << node_name;
    } else {
      FX_LOGS(WARNING) << "RemoveTestNode(" << node_name
                       << ") returned error: " << remove_res.error_value().FormatDescription();
    }
  }
  added_test_nodes_.clear();
}

void DeviceHost::WaitForDeviceNode(const std::string& directory_path,
                                   const std::string& expected_filename) {
  volatile bool found = false;
  std::unique_ptr<fsl::DeviceWatcher> watcher;
  auto deadline = zx::clock::get_monotonic() + zx::sec(20);
  while (!found && zx::clock::get_monotonic() < deadline) {
    if (!watcher) {
      watcher = fsl::DeviceWatcher::Create(
          directory_path,
          [&found, &expected_filename](const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                       const std::string& filename) {
            if (filename == expected_filename) {
              found = true;
            }
          },
          device_loop_.dispatcher());
    }
    device_loop_.RunUntilIdle();
    if (!found) {
      zx::nanosleep(zx::deadline_after(zx::msec(20)));
    }
  }
  ASSERT_NE(watcher, nullptr) << "Failed to open and watch directory " << directory_path;
  ASSERT_TRUE(found) << "Timed out waiting for " << expected_filename << " in " << directory_path;
}

void DeviceHost::AddVirtualDevice(fuchsia::virtualaudio::ControlSyncPtr& controller,
                                  const fuchsia::virtualaudio::DeviceType device_type,
                                  std::optional<bool> is_input) {
  const char* direction;
  if (is_input.has_value()) {
    direction = *is_input ? "input" : "output";
  } else {
    direction = "NONE";
  }
  const char* type;
  switch (device_type) {
    case fuchsia::virtualaudio::DeviceSpecific::Tag::kCodec:
      type = "Codec";
      break;
    case fuchsia::virtualaudio::DeviceSpecific::Tag::kComposite:
      type = "Composite";
      break;
    case fuchsia::virtualaudio::DeviceSpecific::Tag::kDai:
      type = "Dai";
      break;
    case fuchsia::virtualaudio::DeviceSpecific::Tag::kStreamConfig:
      type = "StreamConfig";
      break;
    default:
      ZX_ASSERT(0);
  }
  fuchsia::virtualaudio::Direction configuration_direction;
  if (is_input) {
    configuration_direction.set_is_input(*is_input);
  } else {
    configuration_direction.clear_is_input();
  }

  fuchsia::virtualaudio::Control_GetDefaultConfiguration_Result config_result;
  zx_status_t status = controller->GetDefaultConfiguration(
      device_type, std::move(configuration_direction), &config_result);
  EXPECT_EQ(status, ZX_OK) << "virtualaudio::Control::GetDefaultConfiguration (" << type << " "
                           << direction << ") failed";
  ASSERT_FALSE(config_result.is_err()) << "Failed to GetDefaultConfiguration for (" << type << " "
                                       << direction << ") device: " << config_result.err();

  fuchsia::virtualaudio::Configuration config = std::move(config_result.response().config);
  fuchsia::virtualaudio::Control_AddDevice_Result result;
  auto& device_ptr = virtual_audio_devices_.emplace_back(nullptr);
  status = controller->AddDevice(std::move(config),
                                 device_ptr.NewRequest(device_loop_.dispatcher()), &result);

  EXPECT_EQ(status, ZX_OK) << "virtualaudio::Control::AddDevice (" << type << " " << direction
                           << ") failed";
  ASSERT_FALSE(result.is_err()) << "Failed to add " << type << " " << direction
                                << " device: " << result.err();
  device_ptr.set_error_handler([type, direction](zx_status_t error) {
    FAIL() << "virtualaudio::Device (" << type << " " << direction << ") disconnected: " << error;
  });
}

// Create testcase instances for each device entry.
void DeviceHost::RegisterTests(bool enable_basic_tests, bool enable_admin_tests,
                               bool enable_position_tests) {
  for (auto& device_entry : device_entries()) {
    if (enable_basic_tests) {
      RegisterBasicTestsForDevice(device_entry);
    }
    if (enable_admin_tests) {
      RegisterAdminTestsForDevice(device_entry);
    }
    if (enable_position_tests) {
      RegisterPositionTestsForDevice(device_entry);
    }
  }
}

// Testing is complete. Clean up our virtual audio devices and shut down our loop.
zx_status_t DeviceHost::QuitDeviceLoop() {
  if (shutting_down_) {
    return ZX_OK;
  }
  shutting_down_ = true;

  if (device_loop_.GetState() == ASYNC_LOOP_SHUTDOWN) {
    return ZX_OK;
  }

  libsync::Completion done;
  async::PostTask(device_loop_.dispatcher(), [this, &done]() {
    for (auto& device : virtual_audio_devices_) {
      device.set_error_handler(nullptr);
      if (device.is_bound()) {
        device.Unbind();
      }
    }
    virtual_audio_devices_.clear();
    device_watchers_.clear();
    device_entries_.clear();

    async::PostDelayedTask(
        device_loop_.dispatcher(),
        [this, &done]() {
          if (controller_.is_bound()) {
            zx_status_t status = controller_->RemoveAll();
            EXPECT_EQ(status, ZX_OK) << "Final RemoveAll failed";

            if (status == ZX_OK) {
              uint32_t input_count = -1, output_count = -1, unspecified_direction_count = -1;
              do {
                status = controller_->GetNumDevices(&input_count, &output_count,
                                                    &unspecified_direction_count);
                EXPECT_EQ(status, ZX_OK)
                    << "After final RemoveAll, GetNumDevices (non-legacy) failed";
              } while (status == ZX_OK &&
                       (input_count != 0 || output_count != 0 || unspecified_direction_count != 0));
            }
          }

          if (legacy_controller_.is_bound()) {
            zx_status_t status = legacy_controller_->RemoveAll();
            EXPECT_EQ(status, ZX_OK) << "Final RemoveAll failed";

            if (status == ZX_OK) {
              uint32_t input_count = -1, output_count = -1, unspecified_direction_count = -1;
              do {
                status = legacy_controller_->GetNumDevices(&input_count, &output_count,
                                                           &unspecified_direction_count);
                EXPECT_EQ(status, ZX_OK) << "After final RemoveAll, GetNumDevices (legacy) failed";
              } while (status == ZX_OK &&
                       (input_count != 0 || output_count != 0 || unspecified_direction_count != 0));
            }
          }

          UnregisterVirtualAudioDrivers();

          device_loop_.RunUntilIdle();
          done.Signal();
        },
        zx::msec(100));
  });

  zx_status_t status = done.Wait(zx::sec(20));
  device_loop_.Shutdown();

  return status;
}

}  // namespace media::audio::drivers::test
