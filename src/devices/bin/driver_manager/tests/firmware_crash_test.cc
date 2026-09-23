// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <sdk/lib/component/incoming/cpp/directory.h>
#include <sdk/lib/component/incoming/cpp/protocol.h>

#include "src/devices/bin/driver_manager/firmware_crash/firmware_crash_service.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "zircon/errors.h"
#include "zircon/types.h"

class FirmwareCrashTest : public gtest::TestLoopFixture {};

TEST_F(FirmwareCrashTest, Initialize) {
  driver_manager::FirmwareCrashService service(dispatcher());
}

TEST_F(FirmwareCrashTest, Publish) {
  driver_manager::FirmwareCrashService service(dispatcher());
  component::OutgoingDirectory outgoing(dispatcher());

  service.Publish(outgoing);
}

TEST_F(FirmwareCrashTest, ReportCrash) {
  driver_manager::FirmwareCrashService service(dispatcher());
  component::OutgoingDirectory outgoing(dispatcher());

  service.Publish(outgoing);
  auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  ASSERT_EQ(outgoing.Serve(std::move(root_server)).status_value(), ZX_OK);
  auto svc_client = component::OpenDirectoryAt(root_client, component::kServiceDirectory);
  ASSERT_EQ(svc_client.status_value(), ZX_OK);

  auto reporter_client_end = component::ConnectAt<fuchsia_firmware_crash::Reporter>(*svc_client);
  ASSERT_EQ(reporter_client_end.status_value(), ZX_OK);
  auto reporter_client = fidl::Client(std::move(reporter_client_end.value()), dispatcher());
  auto result = reporter_client->Report({{
      .subsystem_name = "foo",
  }});
  ASSERT_TRUE(result.is_ok());

  RunLoopUntilIdle();
}

TEST_F(FirmwareCrashTest, WatchBlocking) {
  driver_manager::FirmwareCrashService service(dispatcher());
  component::OutgoingDirectory outgoing(dispatcher());

  service.Publish(outgoing);

  auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  ASSERT_EQ(outgoing.Serve(std::move(root_server)).status_value(), ZX_OK);
  auto svc_client = component::OpenDirectoryAt(root_client, component::kServiceDirectory);
  ASSERT_EQ(svc_client.status_value(), ZX_OK);

  auto watcher_client_end = component::ConnectAt<fuchsia_firmware_crash::Watcher>(*svc_client);
  ASSERT_EQ(watcher_client_end.status_value(), ZX_OK);
  auto watcher_client = fidl::Client(std::move(watcher_client_end.value()), dispatcher());

  // Check and see there are no crashes and reply is blocking.
  bool called = false;
  watcher_client->GetCrash().Then([&](auto& result) {
    called = true;
    ASSERT_TRUE(result.is_ok()) << result.error_value().FormatDescription();
    ASSERT_EQ(result.value().subsystem_name(), "foo");
  });
  RunLoopUntilIdle();
  ASSERT_FALSE(called);

  // Report an crash
  auto reporter_client_end = component::ConnectAt<fuchsia_firmware_crash::Reporter>(*svc_client);
  ASSERT_EQ(reporter_client_end.status_value(), ZX_OK);
  auto reporter_client = fidl::Client(std::move(reporter_client_end.value()), dispatcher());
  auto result = reporter_client->Report({{
      .subsystem_name = "foo",
  }});
  ASSERT_TRUE(result.is_ok());
  RunLoopUntilIdle();

  // Check to see if crash was received.
  ASSERT_TRUE(called);
}

TEST_F(FirmwareCrashTest, WatchCountIncreases) {
  driver_manager::FirmwareCrashService service(dispatcher());
  component::OutgoingDirectory outgoing(dispatcher());

  service.Publish(outgoing);

  auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  ASSERT_EQ(outgoing.Serve(std::move(root_server)).status_value(), ZX_OK);
  auto svc_client = component::OpenDirectoryAt(root_client, component::kServiceDirectory);
  ASSERT_EQ(svc_client.status_value(), ZX_OK);

  // Report 3 crashes
  auto reporter_client_end = component::ConnectAt<fuchsia_firmware_crash::Reporter>(*svc_client);
  ASSERT_EQ(reporter_client_end.status_value(), ZX_OK);
  auto reporter_client = fidl::Client(std::move(reporter_client_end.value()), dispatcher());
  auto result = reporter_client->Report({{
      .subsystem_name = "foo",
  }});
  ASSERT_TRUE(result.is_ok());
  result = reporter_client->Report({{
      .subsystem_name = "bar",
  }});
  ASSERT_TRUE(result.is_ok());
  result = reporter_client->Report({{
      .subsystem_name = "foo",
  }});
  ASSERT_TRUE(result.is_ok());
  RunLoopUntilIdle();

  // Receive all 3 calls
  auto watcher_client_end = component::ConnectAt<fuchsia_firmware_crash::Watcher>(*svc_client);
  ASSERT_EQ(watcher_client_end.status_value(), ZX_OK);
  auto watcher_client = fidl::Client(std::move(watcher_client_end.value()), dispatcher());

  uint32_t call_count = 0;
  watcher_client->GetCrash().Then([&](auto& result) {
    call_count++;
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_EQ(result.value().subsystem_name(), "foo");
    ASSERT_EQ(result.value().count(), 1u);
  });
  watcher_client->GetCrash().Then([&](auto& result) {
    call_count++;
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_EQ(result.value().subsystem_name(), "bar");
    ASSERT_EQ(result.value().count(), 1u);
  });
  watcher_client->GetCrash().Then([&](auto& result) {
    call_count++;
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    ASSERT_EQ(result.value().subsystem_name(), "foo");
    ASSERT_EQ(result.value().count(), 2u);
  });
  RunLoopUntilIdle();
  ASSERT_EQ(call_count, 3u);
}
