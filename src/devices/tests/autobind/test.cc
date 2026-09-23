// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/directory.h>

#include <gtest/gtest.h>

TEST(AutobindTest, DriversExist) {
  zx::result channel = device_watcher::RecursiveWaitForFile("/dev/sys/test/autobind");
  ASSERT_EQ(channel.status_value(), ZX_OK);

  // We want to make sure autobind doesn't bind to itself, so try to connect
  // and assert that it was closed.
  zx::channel client, server;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &client, &server));
  ASSERT_EQ(ZX_OK, fdio_service_connect("/dev/sys/test/autobind/autobind", server.release()));
  ASSERT_EQ(ZX_OK, client.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr));
}
