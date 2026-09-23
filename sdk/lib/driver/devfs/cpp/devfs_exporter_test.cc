// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.logger/cpp/wire.h>
#include <fuchsia/device/fs/cpp/fidl_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/component/cpp/tests/test_base.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/fidl/cpp/binding.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

TEST(ConnectorTest, Connect) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  fidl::ServerEnd<fuchsia_logger::LogSink> connection;

  driver_devfs::Connector connector = driver_devfs::Connector<fuchsia_logger::LogSink>(
      [&connection](fidl::ServerEnd<fuchsia_logger::LogSink> server) {
        connection = std::move(server);
      });
  zx::result connector_client = connector.Bind(loop.dispatcher());
  ASSERT_EQ(ZX_OK, connector_client.status_value());

  fidl::WireClient client{std::move(connector_client.value()), loop.dispatcher()};
  auto [_, server_end] = fidl::Endpoints<fuchsia_logger::LogSink>::Create();
  ASSERT_EQ(ZX_OK, client->Connect(server_end.TakeChannel()).status());

  loop.RunUntilIdle();
  ASSERT_TRUE(connection.is_valid());
}
