// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sensors.realm/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>

#include <gtest/gtest.h>

#include "lib/component/incoming/cpp/service.h"
#include "src/sensors/playback/serialization.h"

using fuchsia_component::Namespace;
using fuchsia_component::NamespaceEntry;
using fuchsia_component::NamespaceInputEntry2;
using fuchsia_hardware_sensors::Driver;
using fuchsia_hardware_sensors::Playback;
using fuchsia_hardware_sensors::Service;
using fuchsia_hardware_sensors_realm::RealmFactory;
using fuchsia_math::Vec3F;

using fuchsia_hardware_sensors::ConfigureSensorRateError;
using fuchsia_hardware_sensors::FilePlaybackConfig;
using fuchsia_hardware_sensors::FixedValuesPlaybackConfig;
using fuchsia_hardware_sensors::PlaybackSourceConfig;
using fuchsia_sensors_types::EventPayload;
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorId;
using fuchsia_sensors_types::SensorInfo;
using fuchsia_sensors_types::SensorRateConfig;
using fuchsia_sensors_types::SensorReportingMode;
using fuchsia_sensors_types::SensorType;
using fuchsia_sensors_types::SensorWakeUpType;

using sensors::playback::io::Serializer;

constexpr SensorId kAccelSensorId = 1;
constexpr int kAccelEventLimit = 100;

constexpr SensorId kGyroSensorId = 2;
constexpr int kGyroEventLimit = 10;

namespace {
std::atomic_int32_t namespace_ctr{1};
}  // namespace

/* RealmFactory related boilerplate that will be replaced when there is a C++ client library */
class InstalledNamespace {
 public:
  InstalledNamespace(std::string prefix, zx::channel realm_factory)
      : prefix_(std::move(prefix)), realm_factory_(std::move(realm_factory)) {}
  ~InstalledNamespace() {
    fdio_ns_t* ns;
    EXPECT_EQ(fdio_ns_get_installed(&ns), ZX_OK);
    EXPECT_EQ(fdio_ns_unbind(ns, prefix_.c_str()), ZX_OK);
  }

  template <typename Interface>
  zx_status_t Connect(
      fidl::ServerEnd<Interface> request,
      const std::string& interface_name = fidl::DiscoverableProtocolName<Interface>) const {
    return Connect(interface_name, request.TakeChannel());
  }

  zx_status_t Connect(const std::string& interface_name, zx::channel request) const {
    const std::string path = prefix_ + "/" + interface_name;
    return fdio_service_connect(path.c_str(), request.release());
  }

  std::string prefix() const { return prefix_; }

 private:
  std::string prefix_;
  /// This is not used, but it keeps the RealmFactory connection alive.
  ///
  /// The RealmFactory server may use this connection to pin the lifetime of the realm created
  /// for the test.
  zx::channel realm_factory_;
};
template <typename Interface>
InstalledNamespace ExtendNamespace(zx::eventpair dictionary,
                                   fidl::ClientEnd<Interface> realm_factory) {
  std::string prefix = std::string("/dict-") + std::to_string(namespace_ctr++);

  zx::result namespace_client_end = component::Connect<Namespace>();
  EXPECT_TRUE(namespace_client_end.is_ok());
  fidl::SyncClient<Namespace> ns_client(std::move(*namespace_client_end));

  std::vector<NamespaceInputEntry2> entries;
  entries.emplace_back(NamespaceInputEntry2{{
      .path = prefix,
      .capability = std::move(dictionary),
  }});
  fidl::Result<Namespace::Create2> namespace_create_result = ns_client->Create2(std::move(entries));
  EXPECT_TRUE(namespace_create_result.is_ok());

  std::vector<NamespaceEntry> namespace_entries =
      std::move(namespace_create_result.value().entries());
  EXPECT_EQ(namespace_entries.size(), 1u);
  auto& entry = namespace_entries[0];
  EXPECT_TRUE(entry.path() && entry.directory());
  EXPECT_EQ(entry.path(), prefix);

  fdio_ns_t* ns;
  EXPECT_EQ(fdio_ns_get_installed(&ns), ZX_OK);
  zx_handle_t dir_handle = entry.directory()->TakeChannel().release();
  EXPECT_EQ(fdio_ns_bind(ns, prefix.c_str(), dir_handle), ZX_OK);
  return InstalledNamespace(std::move(prefix), realm_factory.TakeChannel());
}
/* End of RealmFactory boilerplate. */

InstalledNamespace SetupNamespace() {
  zx::result realm_factory_client_end = component::Connect<RealmFactory>();
  EXPECT_TRUE(realm_factory_client_end.is_ok());
  fidl::SyncClient<RealmFactory> realm_factory(std::move(*realm_factory_client_end));

  zx::eventpair dictionary;
  zx::eventpair dictionary_other_end;
  zx_status_t status = zx::eventpair::create(0, &dictionary, &dictionary_other_end);
  EXPECT_EQ(status, ZX_OK);
  fidl::Result<RealmFactory::CreateRealm> create_realm_result =
      realm_factory->CreateRealm(std::move(dictionary));
  EXPECT_TRUE(create_realm_result.is_ok());

  return ExtendNamespace(std::move(dictionary_other_end), realm_factory.TakeClientEnd());
}

std::pair<fidl::ClientEnd<Playback>, fidl::ClientEnd<Driver>> CreateStandardEndpoints(
    InstalledNamespace& test_ns) {
  auto [playback_client_end, playback_server_end] = fidl::Endpoints<Playback>::Create();
  EXPECT_EQ(test_ns.Connect(std::move(playback_server_end)), ZX_OK);
  auto [driver_client_end, driver_server_end] = fidl::Endpoints<Driver>::Create();

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc =
      component::OpenServiceRoot(test_ns.prefix());
  EXPECT_TRUE(svc.is_ok());
  auto service = component::OpenServiceAt<Service>(svc.value(), "playback_sensor_driver");
  EXPECT_TRUE(service.is_ok());
  EXPECT_TRUE(service->connect_driver(std::move(driver_server_end)).is_ok());

  return std::make_pair(std::move(playback_client_end), std::move(driver_client_end));
}

class PlaybackEventHandler : public fidl::AsyncEventHandler<Playback> {
 public:
  PlaybackEventHandler(async::Loop& loop) : loop_(loop) {}

  void handle_unknown_event(fidl::UnknownEventMetadata<Playback> metadata) override {
    FX_LOGS(ERROR) << "Saw unknown event.";
    saw_error_ = true;
    loop_.Quit();
  }

  void on_fidl_error(fidl::UnbindInfo error) override {
    if (expecting_error_) {
      FX_LOGS(INFO) << "Saw aync error: " << error;
    } else {
      FX_LOGS(ERROR) << "Saw async error: " << error;
    }
    saw_error_ = true;
    error_ = error;
    loop_.Quit();
  }

  bool saw_error() { return saw_error_; }
  fidl::UnbindInfo error() { return error_; }
  void reset_error() {
    saw_error_ = false;
    error_ = fidl::UnbindInfo();
  }
  void expecting_error(bool expecting) { expecting_error_ = expecting; }

 private:
  bool expecting_error_ = false;
  bool saw_error_ = false;
  fidl::UnbindInfo error_;
  async::Loop& loop_;
};

class DriverEventHandler : public fidl::AsyncEventHandler<Driver> {
 public:
  DriverEventHandler(async::Loop& loop, uint64_t accel_event_limit = kAccelEventLimit,
                     uint64_t gyro_event_limit = kGyroEventLimit)
      : accel_event_limit_(accel_event_limit), gyro_event_limit_(gyro_event_limit), loop_(loop) {}

  void OnSensorEvent(fidl::Event<Driver::OnSensorEvent>& e) override {
    const SensorEvent& event = e.event();
    sensor_events_[event.sensor_id()].push_back(event);
    event_arrival_timestamps_[event.sensor_id()].push_back(zx::time(zx_clock_get_monotonic()));

    if (sensor_events_[event.sensor_id()].size() == 1) {
      FX_LOGS(INFO) << "Got first event from sensor " << event.sensor_id();
    }

    if (sensor_events_[kAccelSensorId].size() >= accel_event_limit_ &&
        sensor_events_[kGyroSensorId].size() >= gyro_event_limit_ && !saw_final_event_) {
      FX_LOGS(INFO) << "Got final event from sensor " << event.sensor_id();
      loop_.Quit();
      saw_final_event_ = true;
    }
  }

  void handle_unknown_event(fidl::UnknownEventMetadata<Driver> metadata) override {
    FX_LOGS(ERROR) << "Saw unknown event.";
    saw_error_ = true;
    loop_.Quit();
  }

  void on_fidl_error(fidl::UnbindInfo error) override {
    if (expecting_error_) {
      FX_LOGS(INFO) << "Saw aync error: " << error;
    } else {
      FX_LOGS(ERROR) << "Saw async error: " << error;
    }
    saw_error_ = true;
    error_ = error;
    loop_.Quit();
  }

  const std::unordered_map<SensorId, std::vector<SensorEvent>>& sensor_events() {
    return sensor_events_;
  }

  const std::unordered_map<SensorId, std::vector<zx::time>>& event_arrival_timestamps() {
    return event_arrival_timestamps_;
  }

  bool saw_error() { return saw_error_; }
  fidl::UnbindInfo error() { return error_; }
  void reset_error() {
    saw_error_ = false;
    error_ = fidl::UnbindInfo();
  }
  void expecting_error(bool expecting) { expecting_error_ = expecting; }

 private:
  const uint64_t accel_event_limit_;
  const uint64_t gyro_event_limit_;

  bool expecting_error_ = false;
  bool saw_error_ = false;
  fidl::UnbindInfo error_;

  bool saw_final_event_ = false;
  std::unordered_map<SensorId, std::vector<SensorEvent>> sensor_events_;
  std::unordered_map<SensorId, std::vector<zx::time>> event_arrival_timestamps_;
  async::Loop& loop_;
};

SensorInfo CreateAccelerometer(SensorId accel_sensor_id) {
  SensorInfo accelerometer_info;
  accelerometer_info.sensor_id(accel_sensor_id);
  accelerometer_info.name("accelerometer");
  accelerometer_info.vendor("Accelomax");
  accelerometer_info.version(1);
  accelerometer_info.sensor_type(SensorType::kAccelerometer);
  accelerometer_info.wake_up(SensorWakeUpType::kWakeUp);
  accelerometer_info.reporting_mode(SensorReportingMode::kContinuous);
  return accelerometer_info;
}

SensorInfo CreateGyroscope(SensorId gyro_sensor_id) {
  SensorInfo gyroscope_info;
  gyroscope_info.sensor_id(gyro_sensor_id);
  gyroscope_info.name("gyroscope");
  gyroscope_info.vendor("Gyrocorp");
  gyroscope_info.version(1);
  gyroscope_info.sensor_type(SensorType::kGyroscope);
  gyroscope_info.wake_up(SensorWakeUpType::kWakeUp);
  gyroscope_info.reporting_mode(SensorReportingMode::kContinuous);
  return gyroscope_info;
}

void CreateAccelerometerEvents(SensorId accel_sensor_id, int num_events,
                               std::vector<SensorEvent>& event_list) {
  const int64_t kOneMillisecondInNanoseconds = 1e6;
  for (int i = 0; i < num_events; i++) {
    Vec3F sample{{
        .x = i % 3 == 0 ? 1.0f : 0.0f,
        .y = i % 3 == 1 ? 1.0f : 0.0f,
        .z = i % 3 == 2 ? 1.0f : 0.0f,
    }};

    SensorEvent event{{.payload = EventPayload::WithVec3(sample)}};
    event.timestamp() = i * 10 * kOneMillisecondInNanoseconds;
    event.sensor_id() = accel_sensor_id;
    event.sensor_type() = SensorType::kAccelerometer;
    event.sequence_number() = i;

    event_list.push_back(event);
  }
}

void CreateGyroscopeEvents(SensorId gyro_sensor_id, int num_events,
                           std::vector<SensorEvent>& event_list) {
  const int64_t kOneMillisecondInNanoseconds = 1e6;
  for (int i = 0; i < num_events; i++) {
    Vec3F sample{{
        .x = i % 3 == 1 ? 1.0f : 0.0f,
        .y = i % 3 == 2 ? 1.0f : 0.0f,
        .z = i % 3 == 0 ? 1.0f : 0.0f,
    }};

    SensorEvent event{{.payload = EventPayload::WithVec3(sample)}};
    event.timestamp() = i * 100 * kOneMillisecondInNanoseconds;
    event.sensor_id() = gyro_sensor_id;
    event.sensor_type() = SensorType::kGyroscope;
    event.sequence_number() = i;

    event_list.push_back(event);
  }
}

const char kTestDatasetOutputPath[] = "/custom_artifacts/accel_gyro_dataset";
const int kNumUniqueFileAccelEvents = 200;
const int kNumUniqueFileGyroEvents = 20;
void CreateProtoDataFile() {
  // Generate a small dataset and write it to a file.
  const std::string dataset_name = "Test Dataset";
  std::vector<SensorInfo> sensor_list;
  std::vector<SensorEvent> event_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  CreateAccelerometerEvents(kAccelSensorId, kNumUniqueFileAccelEvents, event_list);

  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  CreateGyroscopeEvents(kGyroSensorId, kNumUniqueFileGyroEvents, event_list);

  std::sort(event_list.begin(), event_list.end(), [](const SensorEvent& a, const SensorEvent& b) {
    return a.timestamp() < b.timestamp();
  });
  {
    Serializer serializer(kTestDatasetOutputPath);

    ASSERT_EQ(serializer.Open(dataset_name, sensor_list), ZX_OK);

    for (const SensorEvent& event : event_list) {
      ASSERT_EQ(serializer.WriteEvent(event, event.timestamp()), ZX_OK);
    }
  }
}

const int kNumUniqueFixedAccelEvents = 3;
const int kNumUniqueFixedGyroEvents = 3;
PlaybackSourceConfig CreateFixedValuesPlaybackConfig(std::vector<SensorInfo>& sensor_list) {
  std::vector<SensorEvent> fixed_event_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  CreateAccelerometerEvents(kAccelSensorId, kNumUniqueFixedAccelEvents, fixed_event_list);

  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  CreateGyroscopeEvents(kGyroSensorId, kNumUniqueFixedGyroEvents, fixed_event_list);

  FixedValuesPlaybackConfig fixed_config;
  fixed_config.sensor_list(sensor_list);
  fixed_config.sensor_events(fixed_event_list);

  return PlaybackSourceConfig::WithFixedValuesConfig(fixed_config);
}

const char kTestDatasetInputPath[] = "/pkg/data/accel_gyro_dataset";
PlaybackSourceConfig CreateFilePlaybackConfig() {
  FilePlaybackConfig file_config;
  file_config.file_path(kTestDatasetInputPath);

  return PlaybackSourceConfig::WithFilePlaybackConfig(file_config);
}

struct TimestampDiffHistogram {
  // Number of bins.
  int num_bins;
  // Lower limit of lowest bin.
  int64_t lowest_bin_lower_limit;
  // Upper limit of the highest bin.
  int64_t highest_bin_upper_limit;
  // Width of each bin.
  uint64_t bin_width;

  // Total number of points
  unsigned long total_points;
  // Number of points below all the bins.
  int outliers_below;
  // List of (bin_index, bin_point_count) pairs.
  std::vector<std::pair<int, int>> bin_point_counts;
  // Number of points above all the bins.
  int outliers_above;
};

// Assumes timestamp_diffs is sorted in ascending order.
TimestampDiffHistogram CreateTimestampDiffHistogram(const std::vector<int64_t>& timestamp_diffs,
                                                    int64_t lowest_bin_lower_limit,
                                                    uint64_t bin_width, int num_bins) {
  TimestampDiffHistogram hist;
  hist.num_bins = num_bins;
  hist.lowest_bin_lower_limit = lowest_bin_lower_limit;
  hist.highest_bin_upper_limit = lowest_bin_lower_limit + (bin_width * num_bins);
  hist.bin_width = bin_width;
  hist.total_points = timestamp_diffs.size();
  hist.outliers_below = 0;
  hist.outliers_above = 0;

  int current_bin = 0;
  int current_bin_point_count = 0;
  int64_t current_bin_lower_limit = lowest_bin_lower_limit;
  int64_t current_bin_upper_limit = current_bin_lower_limit + bin_width;
  for (unsigned long i = 0; i < timestamp_diffs.size(); i++) {
    if (timestamp_diffs[i] < lowest_bin_lower_limit) {
      hist.outliers_below += 1;
      continue;
    }
    if (timestamp_diffs[i] >= hist.highest_bin_upper_limit) {
      hist.outliers_above += 1;
      continue;
    }

    if (timestamp_diffs[i] >= current_bin_lower_limit &&
        timestamp_diffs[i] < current_bin_upper_limit) {
      current_bin_point_count += 1;
    } else if (timestamp_diffs[i] >= current_bin_upper_limit) {
      // Store all the points we've counted so far for the bin (if there were any).
      if (current_bin_point_count > 0) {
        hist.bin_point_counts.push_back(std::make_pair(current_bin, current_bin_point_count));
      }
      // Move to the next bin and reconsider this data point.
      current_bin += 1;
      current_bin_point_count = 0;
      current_bin_lower_limit += bin_width;
      current_bin_upper_limit += bin_width;
      i -= 1;
    } else {
      FX_LOGS(ERROR) << "Saw data value out of order when making histogram.";
    }
  }
  if (current_bin_point_count > 0) {
    hist.bin_point_counts.push_back(std::make_pair(current_bin, current_bin_point_count));
  }

  return hist;
}

// Tests that the playback component only allows one Playback protocol client at a time.
TEST(SensorsPlaybackTest, MultiplePlaybackClientsRejected) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  PlaybackEventHandler handler(loop);
  PlaybackEventHandler handler2(loop);
  handler2.expecting_error(true);

  // Create first connection.
  auto test_ns = SetupNamespace();
  auto [playback_client_end, playback_server_end] = fidl::Endpoints<Playback>::Create();
  ASSERT_EQ(test_ns.Connect(std::move(playback_server_end)), ZX_OK);
  fidl::Client playback_client(std::move(playback_client_end), dispatcher, &handler);

  // Run an operation on the connection to make sure this one is actually connected first.
  std::vector<SensorInfo> sensor_list;
  std::optional<bool> result_ok;
  playback_client->ConfigurePlayback(CreateFixedValuesPlaybackConfig(sensor_list))
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_FALSE(handler.saw_error());
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);

  // Create the second connection.
  auto [playback_client_end2, playback_server_end2] = fidl::Endpoints<Playback>::Create();
  ASSERT_EQ(test_ns.Connect(std::move(playback_server_end2)), ZX_OK);
  fidl::Client playback_client2(std::move(playback_client_end2), dispatcher, &handler2);

  // Let the looper run until we get the asynchronous Playback disconnect error.
  loop.Run();
  loop.ResetQuit();
  EXPECT_FALSE(handler.saw_error());
  ASSERT_TRUE(handler2.saw_error());
}

// Tests that the playback component only allows one Driver protocol client at a time.
TEST(SensorsPlaybackTest, MultipleDriverClientsRejected) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  PlaybackEventHandler playback_handler(loop);
  DriverEventHandler handler(loop);
  DriverEventHandler handler2(loop);
  handler2.expecting_error(true);

  // Connect to both protocols and set a valid config just to keep the flow nominal up till
  // the second Driver connection.
  auto test_ns = SetupNamespace();
  auto [playback_client_end, driver_client_end] = CreateStandardEndpoints(test_ns);
  fidl::Client playback_client(std::move(playback_client_end), dispatcher, &playback_handler);
  fidl::Client driver_client(std::move(driver_client_end), dispatcher, &handler);

  std::vector<SensorInfo> sensor_list;
  std::optional<bool> result_ok;
  playback_client->ConfigurePlayback(CreateFixedValuesPlaybackConfig(sensor_list))
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_FALSE(handler.saw_error());
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);

  // Run an operation on the driver connection to make sure it is actually connected first.
  result_ok = std::nullopt;
  handler.reset_error();
  driver_client->GetSensorsList().ThenExactlyOnce(
      [&](fidl::Result<Driver::GetSensorsList>& result) {
        result_ok = result.is_ok();
        if (!result.is_ok()) {
          FX_LOGS(INFO) << "Saw error: " << result.error_value();
        }
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Create the second connection.
  auto [driver_client_end2, driver_server_end2] = fidl::Endpoints<Driver>::Create();
  ASSERT_EQ(test_ns.Connect(std::move(driver_server_end2)), ZX_OK);
  fidl::Client driver_client2(std::move(driver_client_end2), dispatcher, &handler2);

  // Let the looper run until we get the asynchronous Driver disconnect error.
  loop.Run();
  loop.ResetQuit();
  EXPECT_FALSE(handler.saw_error());
  ASSERT_TRUE(handler2.saw_error());
}

// Tests that the playback component disconnects the Driver client if the Playback client
// disconnects.
TEST(SensorsPlaybackTest, DriverDisconnectsIfPlaybackDisconnects) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  DriverEventHandler handler(loop);

  // Connect to the Playback protocol and set a valid playback configuration.
  auto test_ns = SetupNamespace();
  auto [playback_client_end, playback_server_end] = fidl::Endpoints<Playback>::Create();
  ASSERT_EQ(test_ns.Connect(std::move(playback_server_end)), ZX_OK);
  std::optional<fidl::Client<Playback>> playback_client =
      fidl::Client<Playback>(std::move(playback_client_end), dispatcher);

  std::optional<bool> result_ok;
  std::vector<SensorInfo> sensor_list;
  (*playback_client)
      ->ConfigurePlayback(CreateFixedValuesPlaybackConfig(sensor_list))
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });
  loop.Run();
  loop.ResetQuit();
  ASSERT_FALSE(handler.saw_error());
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);

  // Connect to the Driver protocol and call a method to make sure it's connected.
  auto [driver_client_end, driver_server_end] = fidl::Endpoints<Driver>::Create();

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc =
      component::OpenServiceRoot(test_ns.prefix());
  ASSERT_TRUE(svc.is_ok());
  zx::result service = component::OpenServiceAt<Service>(*svc, "playback_sensor_driver");
  ASSERT_TRUE(service.is_ok());
  ASSERT_TRUE(service->connect_driver(std::move(driver_server_end)).is_ok());

  fidl::Client driver_client(std::move(driver_client_end), dispatcher, &handler);

  result_ok = std::nullopt;
  handler.reset_error();
  driver_client->GetSensorsList().ThenExactlyOnce(
      [&](fidl::Result<Driver::GetSensorsList>& result) {
        result_ok = result.is_ok();
        if (!result.is_ok()) {
          FX_LOGS(INFO) << "Saw error: " << result.error_value();
        }
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Destroy the Playback connection. This will cause the playback configuration in the playback
  // component to be cleared and the Driver connection to be closed remotely. Set the handler to
  // expect an asynchronous error (the remote Driver connection closure).
  handler.expecting_error(true);
  playback_client = std::nullopt;

  // Let the looper run until we get the asynchronous Driver disconnect error.
  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(handler.saw_error());
}

// Tests the following features of the playback component:
// - Play back a bunch of fixed valued sensor events provided by the client.
// - Generate "ideal" timestamps based on the configured sampling period.
// This does not test sensor data buffering (max reporting latency is set to 0).
TEST(SensorsPlaybackTest, FixedValues_Unbuffered_IdealScheduling) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  DriverEventHandler handler(loop);

  auto test_ns = SetupNamespace();
  auto [playback_client_end, driver_client_end] = CreateStandardEndpoints(test_ns);

  fidl::Client playback_client(std::move(playback_client_end), dispatcher);
  fidl::Client driver_client(std::move(driver_client_end), dispatcher, &handler);

  // Set fixed playback mode with the generated sensor list and set of events.
  std::vector<SensorInfo> sensor_list;
  std::optional<bool> result_ok = std::nullopt;
  playback_client->ConfigurePlayback(CreateFixedValuesPlaybackConfig(sensor_list))
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Check that we get back the sensor list we just configured.
  result_ok = std::nullopt;
  driver_client->GetSensorsList().ThenExactlyOnce(
      [&](fidl::Result<Driver::GetSensorsList>& result) {
        result_ok = sensor_list == result->sensor_list();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Accelerometer rate.
  SensorRateConfig accel_rate_config;
  accel_rate_config.sampling_period_ns(1e7);      // One hundredth of a second.
  accel_rate_config.max_reporting_latency_ns(0);  // No buffering.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kAccelSensorId, accel_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Gyroscope rate.
  SensorRateConfig gyro_rate_config;
  gyro_rate_config.sampling_period_ns(1e8);      // One tenth of a second.
  gyro_rate_config.max_reporting_latency_ns(0);  // No buffering.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kGyroSensorId, gyro_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Activate the accelerometer.
  std::optional<bool> accel_result_ok;
  driver_client->ActivateSensor({kAccelSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { accel_result_ok = result.is_ok(); });

  // Activate the gyroscope.
  std::optional<bool> gyro_result_ok;
  driver_client->ActivateSensor({kGyroSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { gyro_result_ok = result.is_ok(); });

  // Set deadline of 10 seconds and run.
  FX_LOGS(INFO) << "Waiting for sensor events.";
  zx_status_t run_result = loop.Run(zx::deadline_after(zx::sec(10)));
  loop.ResetQuit();

  if (run_result == ZX_ERR_TIMED_OUT) {
    FX_LOGS(ERROR) << "Timed out waiting for sensor events.";
  }

  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_EQ(run_result, ZX_ERR_CANCELED);
  ASSERT_FALSE(handler.saw_error());
  ASSERT_GE(handler.sensor_events().at(kAccelSensorId).size(), 100ul);
  ASSERT_GE(handler.sensor_events().at(kGyroSensorId).size(), 10ul);

  // Deactivate the accelerometer.
  accel_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kAccelSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        accel_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Deactivate the gyroscope.
  gyro_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kGyroSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        gyro_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Do some statistics on the timestamps to see if they make sense.
  const std::vector<zx::time>& accel_arrival_times =
      handler.event_arrival_timestamps().at(kAccelSensorId);

  // Calculate the differences between all the timestamps, then sort the differences.
  std::vector<int64_t> accel_arrival_diffs;
  for (unsigned long i = 1; i < accel_arrival_times.size(); ++i) {
    zx::duration arrival_diff = accel_arrival_times.at(i) - accel_arrival_times.at(i - 1);
    accel_arrival_diffs.push_back(arrival_diff.get());
  }
  std::sort(accel_arrival_diffs.begin(), accel_arrival_diffs.end());

  // Create a histogram of the timestamp differences. The bins will be 1e7 nanoseconds (0.01
  // seconds) wide. The bottom end of the lowest bin will start at -5e6 nanoseconds (or -0.005
  // seconds) and thus the lowest bin will be centered around 0. There will be 11 bins in total,
  // with the highest bin centered on 1e8 nanoseconds (0.1 seconds) and topping out at 1.05e8
  // nanoseconds (0.105 seconds).
  const int64_t kBottomBinLowerLimit = -5e6;
  const uint64_t kBinWidth = 1e7;
  const int64_t kNumBins = 11;
  TimestampDiffHistogram hist =
      CreateTimestampDiffHistogram(accel_arrival_diffs, kBottomBinLowerLimit, kBinWidth, kNumBins);

  /* The accelerometer is sampling at 100 Hz with no buffering.

     The expected histogram with perfect timing would be this:
     Bin    | 00 | 01 | 02 | 03 | 04 | 05 | 06 | 07 | 08 | 09 | 10 |
     -------*----*----*----*----*----*----*----*----*----*----*----*
     Points | 00 | 99 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 |

     Since buffering is not enabled, the events should be coming evenly spaced by the sampling
     period, which is 1e7 nanoseconds (0.01 seconds). Thus we would expect all 99 data points to
     fall into bin 1.

     Rather than assert that all of the timestamp differences fit exactly into their ideal bins,
     this test will merely assert that bin 1 contains the most points, followed by any outliers
     above. This will ensure the general shape of the timing is correct but reduce flakes by being
     incredibly robust to all but the most massive and repeated test environment performance hiccups
     hiccups (theoretically anyways).
  */

  // Outliers below the bottom bin should not be possible as all of the timestamp differences should
  // be positive.
  ASSERT_EQ(hist.outliers_below, 0);

  // Sort the per-bin point counts in descending order by count.
  std::sort(hist.bin_point_counts.begin(), hist.bin_point_counts.end(),
            [](const std::pair<int, int>& bin1, const std::pair<int, int>& bin2) {
              return bin1.second >= bin2.second;
            });

  // There should be data points in at least one bin.
  ASSERT_GE(hist.bin_point_counts.size(), 1u);
  // Assert the bin with the highest count is bin 1.
  ASSERT_EQ(hist.bin_point_counts[0].first, 1);

  // Outliers above the top bin could conceivably exist but it would be exceptionally unlikely for
  // there to be more of them than data points in bin 1.
  ASSERT_LT(hist.outliers_above, hist.bin_point_counts[0].second);
}

// Tests the following features of the playback component:
// - Play back a bunch of fixed valued sensor events provided by the client.
// - Generate "ideal" timestamps based on the configured sampling period.
// - Simulate hardware buffering of sensor data (use a maximum reporting
//   latency greater than 0).
TEST(SensorsPlaybackTest, FixedValues_Buffered_IdealScheduling) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  DriverEventHandler handler(loop);

  auto test_ns = SetupNamespace();
  auto [playback_client_end, driver_client_end] = CreateStandardEndpoints(test_ns);

  fidl::Client playback_client(std::move(playback_client_end), dispatcher);
  fidl::Client driver_client(std::move(driver_client_end), dispatcher, &handler);

  // Set fixed playback mode with the generated sensor list and set of events.
  std::vector<SensorInfo> sensor_list;
  std::optional<bool> result_ok;
  playback_client->ConfigurePlayback(CreateFixedValuesPlaybackConfig(sensor_list))
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Check that we get back the sensor list we just configured.
  result_ok = std::nullopt;
  driver_client->GetSensorsList().ThenExactlyOnce(
      [&](fidl::Result<Driver::GetSensorsList>& result) {
        result_ok = sensor_list == result->sensor_list();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Accelerometer rate.
  // Set a non-zero maximum reporting latency (one tenth of a second). This will
  // cause the playback component to simulate hardware FIFO buffering.
  SensorRateConfig accel_rate_config;
  accel_rate_config.sampling_period_ns(1e7);        // One hundredth of a second.
  accel_rate_config.max_reporting_latency_ns(1e8);  // One tenth of a second.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kAccelSensorId, accel_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Gyroscope rate.
  SensorRateConfig gyro_rate_config;
  gyro_rate_config.sampling_period_ns(1e8);      // One tenth of a second.
  gyro_rate_config.max_reporting_latency_ns(0);  // No buffering.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kGyroSensorId, gyro_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Activate the accelerometer.
  std::optional<bool> accel_result_ok;
  driver_client->ActivateSensor({kAccelSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { accel_result_ok = result.is_ok(); });

  // Activate the gyroscope.
  std::optional<bool> gyro_result_ok;
  driver_client->ActivateSensor({kGyroSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { gyro_result_ok = result.is_ok(); });

  // Set deadline of 10 seconds and run.
  FX_LOGS(INFO) << "Waiting for sensor events.";
  zx_status_t run_result = loop.Run(zx::deadline_after(zx::sec(10)));
  loop.ResetQuit();

  if (run_result == ZX_ERR_TIMED_OUT) {
    FX_LOGS(ERROR) << "Timed out waiting for sensor events.";
  }

  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_EQ(run_result, ZX_ERR_CANCELED);
  ASSERT_FALSE(handler.saw_error());
  ASSERT_GE(handler.sensor_events().at(kAccelSensorId).size(), 100ul);
  ASSERT_GE(handler.sensor_events().at(kGyroSensorId).size(), 10ul);

  // Deactivate the accelerometer.
  accel_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kAccelSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        accel_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Deactivate the gyroscope.
  gyro_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kGyroSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        gyro_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Do some statistics on the timestamps to see if they make sense.
  const std::vector<zx::time>& accel_arrival_times =
      handler.event_arrival_timestamps().at(kAccelSensorId);

  // Calculate the differences between all the timestamps, then sort the differences.
  std::vector<int64_t> accel_arrival_diffs;
  for (unsigned long i = 1; i < accel_arrival_times.size(); ++i) {
    zx::duration arrival_diff = accel_arrival_times.at(i) - accel_arrival_times.at(i - 1);
    accel_arrival_diffs.push_back(arrival_diff.get());
  }
  std::sort(accel_arrival_diffs.begin(), accel_arrival_diffs.end());

  // Create a histogram of the timestamp differences. The bins will be 1e7 nanoseconds (0.01
  // seconds) wide. The bottom end of the lowest bin will start at -5e6 nanoseconds (or -0.005
  // seconds) and thus the lowest bin will be centered around 0. There will be 11 bins in total,
  // with the highest bin centered on 1e8 nanoseconds (0.1 seconds) and topping out at 1.05e8
  // nanoseconds (0.105 seconds).
  const int64_t kBottomBinLowerLimit = -5e6;
  const uint64_t kBinWidth = 1e7;
  const int64_t kNumBins = 11;
  TimestampDiffHistogram hist =
      CreateTimestampDiffHistogram(accel_arrival_diffs, kBottomBinLowerLimit, kBinWidth, kNumBins);

  /* The accelerometer is sampling at 100 Hz, but with a buffer that is flushed at 10 Hz.

     The expected histogram with perfect timing would be this:
     Bin    | 00 | 01 | 02 | 03 | 04 | 05 | 06 | 07 | 08 | 09 | 10 |
     -------*----*----*----*----*----*----*----*----*----*----*----*
     Points | 90 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 09 |

     Since buffering is enabled, the events are coming in 10 bursts of 10. The timestamp difference
     between events in a burst will be very small, and thus we expect bin 0 to contain the most data
     points (90 ideally). In the 9 gaps between the 10 bursts we expect to see timestamp differences
     close to 0.1 seconds, and so ideally there should be 9 data points in bin 10.

     Rather than assert that all of the timestamp differences fit exactly into their ideal bins,
     this test will merely assert that bin 0 contains the most points, followed by bin 10, followed
     by any other bin or outliers above. This will ensure the general shape of the timing is correct
     but reduce flakes by being incredibly robust to all but the most massive and repeated test
     environment performance hiccups (theoretically anyways).
   */

  // Outliers below the bottom bin should not be possible as all of the timestamp differences should
  // be positive.
  ASSERT_EQ(hist.outliers_below, 0);

  // Sort the per-bin point counts in descending order by count.
  std::sort(hist.bin_point_counts.begin(), hist.bin_point_counts.end(),
            [](const std::pair<int, int>& bin1, const std::pair<int, int>& bin2) {
              return bin1.second >= bin2.second;
            });

  // There should be data points in at least two bins.
  ASSERT_GE(hist.bin_point_counts.size(), 2u);
  // Assert the bin with the highest count is bin 0.
  ASSERT_EQ(hist.bin_point_counts[0].first, 0);
  // Assert the bin with the next highest couint is bin 10.
  ASSERT_EQ(hist.bin_point_counts[1].first, 10);
}

// Uncomment this locally and run fx test with --ffx-output-directory <output_dir> in
// order to regenerate the file that the ProtoFile tests below expect.
TEST(SensorsPlaybackTest, DISABLED_GenerateTestDatasetProtoFile) { CreateProtoDataFile(); }

// Tests the following features of the playback component:
// - Play back data recorded to a binary protobuf file.
// - Uses the "receipt timestamps" of events in the dataset as "presentation timestamps" during
//   playback.
// This does not test sensor data buffering (max reporting latency is set to 0).
TEST(SensorsPlaybackTest, ProtoFile_Unbuffered_PresentationTimeScheduling) {
  // Expected sensor data.
  std::vector<SensorInfo> sensor_list;
  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  sensor_list.push_back(CreateGyroscope(kGyroSensorId));

  // Play back the data from the file.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  // Increase the event limits in the handler to make sure the file wraps a few times.
  DriverEventHandler handler(loop, 1000, 100);

  auto test_ns = SetupNamespace();
  auto [playback_client_end, driver_client_end] = CreateStandardEndpoints(test_ns);

  fidl::Client playback_client(std::move(playback_client_end), dispatcher);
  fidl::Client driver_client(std::move(driver_client_end), dispatcher, &handler);

  std::optional<bool> result_ok;
  playback_client->ConfigurePlayback(CreateFilePlaybackConfig())
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Check that we get the sensor list we expect from the file.
  result_ok = std::nullopt;
  std::vector<SensorInfo> result_list;
  driver_client->GetSensorsList().ThenExactlyOnce(
      [&](fidl::Result<Driver::GetSensorsList>& result) {
        result_list = result->sensor_list();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_FALSE(handler.saw_error());
  ASSERT_EQ(sensor_list, result_list);

  // Accelerometer rate. We expect the sampling period to be ignored.
  SensorRateConfig accel_rate_config;
  accel_rate_config.sampling_period_ns(1e7);      // One hundredth of a second.
  accel_rate_config.max_reporting_latency_ns(0);  // No buffering.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kAccelSensorId, accel_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Gyroscope rate. We expect the sampling period to be ignored.
  SensorRateConfig gyro_rate_config;
  gyro_rate_config.sampling_period_ns(1e8);      // One tenth of a second.
  gyro_rate_config.max_reporting_latency_ns(0);  // No buffering.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kGyroSensorId, gyro_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Activate the accelerometer.
  std::optional<bool> accel_result_ok;
  driver_client->ActivateSensor({kAccelSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { accel_result_ok = result.is_ok(); });

  // Activate the gyroscope.
  std::optional<bool> gyro_result_ok;
  driver_client->ActivateSensor({kGyroSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { gyro_result_ok = result.is_ok(); });

  // Set deadline of 20 seconds and run.
  FX_LOGS(INFO) << "Waiting for sensor events.";
  zx_status_t run_result = loop.Run(zx::deadline_after(zx::sec(20)));
  loop.ResetQuit();

  if (run_result == ZX_ERR_TIMED_OUT) {
    FX_LOGS(ERROR) << "Timed out waiting for sensor events.";
  }

  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_EQ(run_result, ZX_ERR_CANCELED);
  ASSERT_FALSE(handler.saw_error());
  ASSERT_GE(handler.sensor_events().at(kAccelSensorId).size(), 100ul);
  ASSERT_GE(handler.sensor_events().at(kGyroSensorId).size(), 10ul);

  // Deactivate the accelerometer.
  accel_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kAccelSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        accel_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Deactivate the gyroscope.
  gyro_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kGyroSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        gyro_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Do some statistics on the timestamps to see if they make sense.
  const std::vector<zx::time>& accel_arrival_times =
      handler.event_arrival_timestamps().at(kAccelSensorId);

  // Calculate the differences between all the timestamps, then sort the differences.
  std::vector<int64_t> accel_arrival_diffs;
  for (unsigned long i = 1; i < accel_arrival_times.size(); ++i) {
    zx::duration arrival_diff = accel_arrival_times.at(i) - accel_arrival_times.at(i - 1);
    accel_arrival_diffs.push_back(arrival_diff.get());
  }
  std::sort(accel_arrival_diffs.begin(), accel_arrival_diffs.end());

  // Create a histogram of the timestamp differences. The bins will be 1e7 nanoseconds (0.01
  // seconds) wide. The bottom end of the lowest bin will start at -5e6 nanoseconds (or -0.005
  // seconds) and thus the lowest bin will be centered around 0. There will be 11 bins in total,
  // with the highest bin centered on 1e8 nanoseconds (0.1 seconds) and topping out at 1.05e8
  // nanoseconds (0.105 seconds).
  const int64_t kBottomBinLowerLimit = -5e6;
  const uint64_t kBinWidth = 1e7;
  const int64_t kNumBins = 11;
  TimestampDiffHistogram hist =
      CreateTimestampDiffHistogram(accel_arrival_diffs, kBottomBinLowerLimit, kBinWidth, kNumBins);

  /* The accelerometer is sampling at 100 Hz with no buffering.

     The expected histogram with perfect timing would be this:
     Bin    | 00 | 01 | 02 | 03 | 04 | 05 | 06 | 07 | 08 | 09 | 10 |
     -------*----*----*----*----*----*----*----*----*----*----*----*
     Points | 00 | 99 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 |

     Since buffering is not enabled, the events should be coming evenly spaced by the sampling
     period, which is 1e7 nanoseconds (0.01 seconds). Thus we would expect all 99 data points to
     fall into bin 1.

     Rather than assert that all of the timestamp differences fit exactly into their ideal bins,
     this test will merely assert that bin 1 contains the most points, followed by any outliers
     above. This will ensure the general shape of the timing is correct but reduce flakes by being
     incredibly robust to all but the most massive and repeated test environment performance hiccups
     hiccups (theoretically anyways).
   */

  // Outliers below the bottom bin should not be possible as all of the timestamp differences should
  // be positive.
  ASSERT_EQ(hist.outliers_below, 0);

  // Sort the per-bin point counts in descending order by count.
  std::sort(hist.bin_point_counts.begin(), hist.bin_point_counts.end(),
            [](const std::pair<int, int>& bin1, const std::pair<int, int>& bin2) {
              return bin1.second >= bin2.second;
            });

  // There should be data points in at least one bin.
  ASSERT_GE(hist.bin_point_counts.size(), 1u);
  // Assert the bin with the highest count is bin 1.
  ASSERT_EQ(hist.bin_point_counts[0].first, 1);

  // Outliers above the top bin could conceivably exist but it would be exceptionally unlikely for
  // there to be more of them than data points in bin 1.
  ASSERT_LT(hist.outliers_above, hist.bin_point_counts[0].second);
}

// Tests the following features of the playback component:
// - Play back data recorded to a binary protobuf file.
// - Uses the "receipt timestamps" of events in the dataset as "presentation timestamps" during
//   playback.
// - Simulate hardware buffering of sensor data (use a maximum reporting
//   latency greater than 0).
TEST(SensorsPlaybackTest, ProtoFile_Buffered_PresentationTimeScheduling) {
  // Expected sensor data.
  std::vector<SensorInfo> sensor_list;
  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  sensor_list.push_back(CreateGyroscope(kGyroSensorId));

  // Play back the data from the file.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  // Increase the event limits in the handler to make sure the file wraps a few times.
  DriverEventHandler handler(loop, 1000, 100);

  auto test_ns = SetupNamespace();
  auto [playback_client_end, driver_client_end] = CreateStandardEndpoints(test_ns);

  fidl::Client playback_client(std::move(playback_client_end), dispatcher);
  fidl::Client driver_client(std::move(driver_client_end), dispatcher, &handler);

  std::optional<bool> result_ok;
  playback_client->ConfigurePlayback(CreateFilePlaybackConfig())
      .ThenExactlyOnce([&](fidl::Result<Playback::ConfigurePlayback>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Check that we get the sensor list we expect from the file.
  result_ok = std::nullopt;
  std::vector<SensorInfo> result_list;
  driver_client->GetSensorsList().ThenExactlyOnce(
      [&](fidl::Result<Driver::GetSensorsList>& result) {
        result_list = result->sensor_list();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_FALSE(handler.saw_error());
  ASSERT_EQ(sensor_list, result_list);

  // Accelerometer rate. We expect the sampling period to be ignored.
  SensorRateConfig accel_rate_config;
  accel_rate_config.sampling_period_ns(1e7);        // One hundredth of a second.
  accel_rate_config.max_reporting_latency_ns(1e8);  // One tenth of a second.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kAccelSensorId, accel_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Gyroscope rate. We expect the sampling period to be ignored.
  SensorRateConfig gyro_rate_config;
  gyro_rate_config.sampling_period_ns(1e8);      // One tenth of a second.
  gyro_rate_config.max_reporting_latency_ns(0);  // No buffering.

  result_ok = std::nullopt;
  driver_client->ConfigureSensorRate({kGyroSensorId, gyro_rate_config})
      .ThenExactlyOnce([&](fidl::Result<Driver::ConfigureSensorRate>& result) {
        result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(result_ok.has_value());
  ASSERT_TRUE(*result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Activate the accelerometer.
  std::optional<bool> accel_result_ok;
  driver_client->ActivateSensor({kAccelSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { accel_result_ok = result.is_ok(); });

  // Activate the gyroscope.
  std::optional<bool> gyro_result_ok;
  driver_client->ActivateSensor({kGyroSensorId})
      .ThenExactlyOnce(
          [&](fidl::Result<Driver::ActivateSensor>& result) { gyro_result_ok = result.is_ok(); });

  // Set deadline of 10 seconds and run.
  FX_LOGS(INFO) << "Waiting for sensor events.";
  zx_status_t run_result = loop.Run(zx::deadline_after(zx::sec(20)));
  loop.ResetQuit();

  if (run_result == ZX_ERR_TIMED_OUT) {
    FX_LOGS(ERROR) << "Timed out waiting for sensor events.";
  }

  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_EQ(run_result, ZX_ERR_CANCELED);
  ASSERT_FALSE(handler.saw_error());
  ASSERT_GE(handler.sensor_events().at(kAccelSensorId).size(), 100ul);
  ASSERT_GE(handler.sensor_events().at(kGyroSensorId).size(), 10ul);

  // Deactivate the accelerometer.
  accel_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kAccelSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        accel_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(accel_result_ok.has_value());
  ASSERT_TRUE(*accel_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Deactivate the gyroscope.
  gyro_result_ok = std::nullopt;
  driver_client->DeactivateSensor({kGyroSensorId})
      .ThenExactlyOnce([&](fidl::Result<Driver::DeactivateSensor>& result) {
        gyro_result_ok = result.is_ok();
        loop.Quit();
      });

  loop.Run();
  loop.ResetQuit();
  ASSERT_TRUE(gyro_result_ok.has_value());
  ASSERT_TRUE(*gyro_result_ok);
  ASSERT_FALSE(handler.saw_error());

  // Do some statistics on the timestamps to see if they make sense.
  const std::vector<zx::time>& accel_arrival_times =
      handler.event_arrival_timestamps().at(kAccelSensorId);

  // Calculate the differences between all the timestamps, then sort the differences.
  std::vector<int64_t> accel_arrival_diffs;
  for (unsigned long i = 1; i < accel_arrival_times.size(); ++i) {
    zx::duration arrival_diff = accel_arrival_times.at(i) - accel_arrival_times.at(i - 1);
    accel_arrival_diffs.push_back(arrival_diff.get());
  }
  std::sort(accel_arrival_diffs.begin(), accel_arrival_diffs.end());

  // Create a histogram of the timestamp differences. The bins will be 1e7 nanoseconds (0.01
  // seconds) wide. The bottom end of the lowest bin will start at -5e6 nanoseconds (or -0.005
  // seconds) and thus the lowest bin will be centered around 0. There will be 11 bins in total,
  // with the highest bin centered on 1e8 nanoseconds (0.1 seconds) and topping out at 1.05e8
  // nanoseconds (0.105 seconds).
  const int64_t kBottomBinLowerLimit = -5e6;
  const uint64_t kBinWidth = 1e7;
  const int64_t kNumBins = 11;
  TimestampDiffHistogram hist =
      CreateTimestampDiffHistogram(accel_arrival_diffs, kBottomBinLowerLimit, kBinWidth, kNumBins);

  /* The accelerometer is sampling at 100 Hz, but with a buffer that is flushed at 10 Hz.

     The expected histogram with perfect timing would be this:
     Bin    | 00 | 01 | 02 | 03 | 04 | 05 | 06 | 07 | 08 | 09 | 10 |
     -------*----*----*----*----*----*----*----*----*----*----*----*
     Points | 90 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 00 | 09 |

     Since buffering is enabled, the events are coming in 10 bursts of 10. The timestamp difference
     between events in a burst will be very small, and thus we expect bin 0 to contain the most data
     points (90 ideally). In the 9 gaps between the 10 bursts we expect to see timestamp differences
     close to 0.1 seconds, and so ideally there should be 9 data points in bin 10.

     Rather than assert that all of the timestamp differences fit exactly into their ideal bins,
     this test will merely assert that bin 0 contains the most points, followed by bin 10, followed
     by any other bin or outliers above. This will ensure the general shape of the timing is correct
     but reduce flakes by being incredibly robust to all but the most massive and repeated test
     environment performance hiccups (theoretically anyways).
   */

  // Outliers below the bottom bin should not be possible as all of the timestamp differences should
  // be positive.
  ASSERT_EQ(hist.outliers_below, 0);

  // Sort the per-bin point counts in descending order by count.
  std::sort(hist.bin_point_counts.begin(), hist.bin_point_counts.end(),
            [](const std::pair<int, int>& bin1, const std::pair<int, int>& bin2) {
              return bin1.second >= bin2.second;
            });

  // There should be data points in at least two bins.
  ASSERT_GE(hist.bin_point_counts.size(), 2u);
  // Assert the bin with the highest count is bin 0.
  ASSERT_EQ(hist.bin_point_counts[0].first, 0);
  // Assert the bin with the next highest couint is bin 10.
  ASSERT_EQ(hist.bin_point_counts[1].first, 10);
}
