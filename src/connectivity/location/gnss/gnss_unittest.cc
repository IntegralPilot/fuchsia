// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "gnss_service.h"
#include "src/lib/testing/predicates/status.h"

namespace {
void CheckReceivedLocation(const fuchsia_location_gnss_types::Location& received_location,
                           const fuchsia_location_gnss_types::Location& expected_location) {
  EXPECT_EQ(received_location.lat_long()->lat_long()->latitude_deg(),
            expected_location.lat_long()->lat_long()->latitude_deg());
  EXPECT_EQ(received_location.lat_long()->lat_long()->longitude_deg(),
            expected_location.lat_long()->lat_long()->longitude_deg());
  EXPECT_EQ(received_location.lat_long()->horizontal_accuracy_meters(),
            expected_location.lat_long()->horizontal_accuracy_meters());
}
}  // namespace

namespace gnss {
namespace testing {

class GnssServiceTest : public ::testing::Test {
 public:
  GnssServiceTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override {
    SetUpFakeLocations();
    zx_status_t start_status = loop_.StartThread("GnssServiceTestLoop");
    ZX_ASSERT_MSG(start_status == ZX_OK, "loop_.StartThread() failed: %s",
                  zx_status_get_string(start_status));
    gnss_service_ = std::make_shared<GnssService>(loop_.dispatcher());
  }

  void TearDown() override {
    libsync::Completion binding_closed;
    // Ensure the service destructor runs on the dispatcher thread to avoid any
    // race conditions. gnss_service_.reset() will close all bindings.
    async::PostTask(loop_.dispatcher(), [&]() {
      if (gnss_service_) {
        gnss_service_.reset();
      }
      binding_closed.Signal();
    });
    binding_closed.Wait();
    loop_.Shutdown();
  }

  fidl::ClientEnd<fuchsia_location_gnss::Provider> ConnectClient() {
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_location_gnss::Provider>();
    EXPECT_OK(endpoints) << "Failed to create endpoints: " << endpoints.status_string();
    if (!endpoints.is_ok()) {
      return {};  // Return an unbound client on failure
    }
    std::shared_ptr<gnss::GnssService> gnss_service_to_bind = gnss_service_;
    auto [client_end, server_end] = std::move(endpoints.value());
    zx_status_t post_status = async::PostTask(
        loop_.dispatcher(), [server_end = std::move(server_end), gnss_service_to_bind,
                             dispatcher = loop_.dispatcher()]() mutable {
          if (gnss_service_to_bind) {
            gnss_service_to_bind->AddBinding(dispatcher, std::move(server_end));
          } else {
            server_end.Close(ZX_ERR_INTERNAL);
          }
        });
    EXPECT_OK(post_status) << "Failed to post binding task: " << zx_status_get_string(post_status);
    if (post_status != ZX_OK) {
      return {};  // Return an unbound client on failure
    }
    return std::move(client_end);
  }

  void SendLocationUpdate(const fuchsia_location_gnss_types::Location& location,
                          zx::duration delay = zx::duration(0)) {
    async::PostDelayedTask(
        loop_.dispatcher(), [&]() mutable { gnss_service_->SendUpdateToListeners(location); },
        delay);
  }

 protected:
  async::Loop loop_;
  std::shared_ptr<gnss::GnssService> gnss_service_;
  std::vector<fuchsia_location_gnss_types::Location> fake_locations_;

  // Define two fake locations for testing.
  void SetUpFakeLocations() {
    fuchsia_location_gnss_types::Location loc1;
    fuchsia_location_gnss_types::HorizontalLocation lat_long_with_hepe1;
    fuchsia_location_gnss_types::LatLong lat_long1;
    lat_long1.latitude_deg(37.410477).longitude_deg(-121.953670);
    lat_long_with_hepe1.lat_long(lat_long1).horizontal_accuracy_meters(9);
    loc1.lat_long(lat_long_with_hepe1);
    fake_locations_.push_back(loc1);

    fuchsia_location_gnss_types::Location loc2;
    fuchsia_location_gnss_types::HorizontalLocation lat_long_with_hepe2;
    fuchsia_location_gnss_types::LatLong lat_long2;
    lat_long2.latitude_deg(34.0522).longitude_deg(-118.2437);
    lat_long_with_hepe2.lat_long(lat_long2).horizontal_accuracy_meters(10);
    loc2.lat_long(lat_long_with_hepe2);
    fake_locations_.push_back(loc2);
  }
};

TEST_F(GnssServiceTest, GetCapabilities) {
  fidl::SyncClient<fuchsia_location_gnss::Provider> client(ConnectClient());
  ASSERT_TRUE(client.is_valid());
  auto received_caps = client->GetCapabilities();
  ASSERT_TRUE(received_caps.is_ok()) << received_caps.error_value();
  EXPECT_EQ(static_cast<uint32_t>(fuchsia_location_gnss_types::Capabilities::kCapabilityScheduling |
                                  fuchsia_location_gnss_types::Capabilities::kCapabilitySingleShot |
                                  fuchsia_location_gnss_types::Capabilities::kCapabilityMsa),
            static_cast<uint32_t>(received_caps.value().capabilities()));
}

TEST_F(GnssServiceTest, StartTimeBasedLocationTracking) {
  fidl::SyncClient<fuchsia_location_gnss::Provider> client(ConnectClient());
  ASSERT_TRUE(client.is_valid());

  // Create client and server endpoints for the LocationListener protocol.
  zx::result listener_endpoints = fidl::CreateEndpoints<fuchsia_location_gnss::LocationListener>();
  ASSERT_OK(listener_endpoints) << listener_endpoints.status_string();
  auto [listener_client_end, listener_server_end] = std::move(listener_endpoints.value());

  auto start_tracking_result = client->StartTimeBasedLocationTracking(
      fuchsia_location_gnss::ProviderStartTimeBasedLocationTrackingRequest(
          fuchsia_location_gnss_types::FixParams(
              {.fix_type = fuchsia_location_gnss_types::FixType::kMsAssisted,
               .max_time_secs = 60,
               .max_dist_meters = 5}),
          5000, std::move(listener_server_end)));
  ASSERT_TRUE(start_tracking_result.is_ok()) << start_tracking_result.error_value();

  // Check that the listener client end is still valid.
  ASSERT_TRUE(listener_client_end.is_valid());

  std::optional<fuchsia_location_gnss_types::Location> received_location;
  libsync::Completion location_received_completion;
  zx_status_t wait_status;

  fidl::SharedClient<fuchsia_location_gnss::LocationListener> listener_client(
      std::move(listener_client_end), loop_.dispatcher());
  ASSERT_TRUE(listener_client.is_valid());

  // Update location in GNSS service and call GetNextLocation on listener.
  // GetNextLocation call will immediately return with updated location value.
  SendLocationUpdate(fake_locations_[0]);

  listener_client->GetNextLocation().Then(
      [&](fidl::Result<fuchsia_location_gnss::LocationListener::GetNextLocation>& result) {
        if (result.is_ok()) {
          received_location = result.value().location();
        } else {
          FX_LOGS(ERROR) << "GetSingleShotFix FIDL call failed: " << result.error_value();
        }
        // Signal that the callback has completed (successfully or with error).
        location_received_completion.Signal();
      });

  location_received_completion.Wait();
  ASSERT_TRUE(received_location.has_value()) << "Callback completed but location was not received.";
  CheckReceivedLocation(received_location.value(), fake_locations_[0]);
  location_received_completion.Reset();

  // Send next hanging get call GetNextLocation. Try waiting for 5 seconds. No location reported.
  listener_client->GetNextLocation().Then(
      [&](fidl::Result<fuchsia_location_gnss::LocationListener::GetNextLocation>& result) {
        if (result.is_ok()) {
          received_location = result.value().location();
        } else {
          FX_LOGS(ERROR) << "GetSingleShotFix FIDL call failed: " << result.error_value();
        }
        // Signal that the callback has completed (successfully or with error).
        location_received_completion.Signal();
      });
  wait_status = location_received_completion.Wait(zx::sec(5));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, wait_status);

  // Update service with old location. This should not trigger a location update.
  SendLocationUpdate(fake_locations_[0]);
  wait_status = location_received_completion.Wait(zx::sec(5));
  ASSERT_EQ(ZX_ERR_TIMED_OUT, wait_status);

  // Update service with new location and wait for listener to get notified.
  SendLocationUpdate(fake_locations_[1]);
  location_received_completion.Wait();

  ASSERT_TRUE(received_location.has_value()) << "Callback completed but location was not received.";
  CheckReceivedLocation(received_location.value(), fake_locations_[1]);
  location_received_completion.Reset();
}

TEST_F(GnssServiceTest, GetSingleShotFixAsync) {
  fidl::SharedClient<fuchsia_location_gnss::Provider> client(ConnectClient(), loop_.dispatcher());
  ASSERT_TRUE(client.is_valid());

  std::optional<fuchsia_location_gnss_types::Location> received_location;
  libsync::Completion location_received_completion;

  client
      ->GetSingleShotFix(fuchsia_location_gnss_types::FixParams(
          {.fix_type = fuchsia_location_gnss_types::FixType::kMsAssisted,
           .max_time_secs = 60,
           .max_dist_meters = 5}))
      .Then([&](fidl::Result<fuchsia_location_gnss::Provider::GetSingleShotFix>& result) {
        if (result.is_ok()) {
          received_location = result.value().location();
        } else {
          FX_LOGS(ERROR) << "GetSingleShotFix FIDL call failed: " << result.error_value();
        }
        // Signal that the callback has completed (successfully or with error).
        location_received_completion.Signal();
      });

  SendLocationUpdate(fake_locations_[0], zx::msec(50));

  location_received_completion.Wait();

  ASSERT_TRUE(received_location.has_value()) << "Callback completed but location was not received.";

  CheckReceivedLocation(received_location.value(), fake_locations_[0]);
}

}  // namespace testing
}  // namespace gnss
