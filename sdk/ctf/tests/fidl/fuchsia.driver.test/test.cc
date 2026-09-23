// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>

#include <zxtest/zxtest.h>

namespace {

TEST(DriverTestRealmCts, DriverWasLoaded) {
  // Connect to DriverTestRealm.
  auto dtr_client_result = component::Connect<fuchsia_driver_test::Realm>();
  ASSERT_OK(dtr_client_result);
  fidl::SyncClient<fuchsia_driver_test::Realm> dtr_client(std::move(dtr_client_result.value()));

  // Start the DriverTestRealm.
  fuchsia_driver_test::RealmArgs args{};
  fidl::Result<fuchsia_driver_test::Realm::Start> start_result = dtr_client->Start(std::move(args));
  ASSERT_TRUE(start_result.is_ok());

  // Connect to the driver development manager.
  auto fdd_client_result = component::Connect<fuchsia_driver_development::Manager>();
  ASSERT_OK(fdd_client_result);
  fidl::SyncClient<fuchsia_driver_development::Manager> fdd_client(
      std::move(fdd_client_result.value()));

  // Keep checking nodes until we see dev.sys.test which is created by the root driver.
  while (true) {
    auto node_info_iter_endpoints_result =
        fidl::CreateEndpoints<fuchsia_driver_development::NodeInfoIterator>();
    ASSERT_OK(node_info_iter_endpoints_result);

    // Filtering for an exact match of the node moniker.
    auto get_node_info_result = fdd_client->GetNodeInfo({{
        .node_filter = {"dev.sys.test"},
        .iterator = std::move(node_info_iter_endpoints_result.value().server),
        .exact_match = true,
    }});
    ASSERT_TRUE(get_node_info_result.is_ok());

    fidl::SyncClient<fuchsia_driver_development::NodeInfoIterator> node_info_iter_client(
        std::move(node_info_iter_endpoints_result.value().client));

    auto node_info_result = node_info_iter_client->GetNext();
    ASSERT_TRUE(node_info_result.is_ok());

    // Since we filtered for an exact match, there should only be 1 when it is found.
    if (node_info_result.value().nodes().size() == 1) {
      // Makes sure the filtering was correct.
      ASSERT_EQ(node_info_result.value().nodes()[0].moniker().value(), "dev.sys.test");
      break;
    }
  }
}

}  // namespace
