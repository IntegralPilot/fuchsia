// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests should run without any network interface (except loopback).

#include <fidl/fuchsia.device/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <sys/utsname.h>

#include <gtest/gtest.h>

#include "src/bringup/bin/device-name-provider/tests/integration_test_config.h"
#include "src/lib/testing/predicates/status.h"

namespace {

const char* kDeviceNameWithMAC = "fuchsia-4567-89ab-cdef";
const bool kUseBootItem = integration_test_config::Config::TakeFromStartupHandle().use_boot_item();

TEST(NameProviderTest, GetHostNameDefault) {
  char hostname[HOST_NAME_MAX];
  ASSERT_EQ(gethostname(hostname, sizeof(hostname)), 0) << strerror(errno);
  if (kUseBootItem) {
    ASSERT_STREQ(hostname, kDeviceNameWithMAC);
  } else {
    ASSERT_STREQ(hostname, fuchsia_device::wire::kDefaultDeviceName);
  }
}

TEST(NameProviderTest, UnameDefault) {
  utsname uts;
  ASSERT_EQ(uname(&uts), 0) << strerror(errno);
  if (kUseBootItem) {
    ASSERT_STREQ(uts.nodename, kDeviceNameWithMAC);
  } else {
    ASSERT_STREQ(uts.nodename, fuchsia_device::wire::kDefaultDeviceName);
  }
}

TEST(NameProviderTest, GetDeviceName) {
  zx::result client_end = component::Connect<fuchsia_device::NameProvider>();
  ASSERT_OK(client_end.status_value());

  fidl::WireResult response = fidl::WireCall(client_end.value())->GetDeviceName();
  ASSERT_OK(response.status());
  const auto* res = response.Unwrap();
  if (res->is_error()) {
    FAIL() << zx_status_get_string(res->error_value());
  }

  const fidl::StringView& name = res->value()->name;
  // regression test: ensure that no additional data is present past the last null byte
  if (kUseBootItem) {
    EXPECT_EQ(name.size(), strlen(kDeviceNameWithMAC));
    EXPECT_EQ(memcmp(name.data(), kDeviceNameWithMAC, name.size()), 0);
  } else {
    EXPECT_EQ(name.size(), strlen(fuchsia_device::wire::kDefaultDeviceName));
    EXPECT_EQ(memcmp(name.data(), fuchsia_device::wire::kDefaultDeviceName, name.size()), 0);
  }
}

}  // namespace
