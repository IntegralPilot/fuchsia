// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_POWER_TESTING_CLIENT_CPP_POWER_FRAMEWORK_TEST_REALM_H_
#define SRC_POWER_TESTING_CLIENT_CPP_POWER_FRAMEWORK_TEST_REALM_H_

#include <fidl/fuchsia.hardware.power.suspend/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>

namespace power_framework_test_realm {

void Setup(component_testing::RealmBuilder& realm_builder);

zx::result<fidl::ClientEnd<fuchsia_hardware_power_suspend::Suspender>> ConnectToSuspender(
    component_testing::RealmRoot& root);

}  // namespace power_framework_test_realm

#endif  // SRC_POWER_TESTING_CLIENT_CPP_POWER_FRAMEWORK_TEST_REALM_H_
