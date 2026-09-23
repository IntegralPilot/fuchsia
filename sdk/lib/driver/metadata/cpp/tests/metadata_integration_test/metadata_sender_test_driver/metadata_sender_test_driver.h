// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_METADATA_CPP_TESTS_METADATA_INTEGRATION_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_SENDER_TEST_DRIVER_H_
#define LIB_DRIVER_METADATA_CPP_TESTS_METADATA_INTEGRATION_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_SENDER_TEST_DRIVER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/node/cpp/add_child.h>

namespace fdf_metadata::test {

// This driver's purpose is to serve metadata to its two child nodes using `fdf::MetadataServer`.
class MetadataSenderTestDriver : public fdf::DriverBase2,
                                 public fidl::Server<fuchsia_hardware_test::MetadataSender> {
 public:
  static constexpr std::string_view kDriverName = "sender";

  MetadataSenderTestDriver() : DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

  // fidl::Server<fuchsia_hardware_test::MetadataSender> implementation.
  void ServeMetadata(ServeMetadataRequest& request,
                     ServeMetadataCompleter::Sync& completer) override;
  void AddMetadataRetrieverNode(AddMetadataRetrieverNodeRequest& request,
                                AddMetadataRetrieverNodeCompleter::Sync& completer) override;
  void AddMetadataForwarderNode(AddMetadataForwarderNodeCompleter::Sync& completer) override;

 private:
  // Helper function that adds a child node with the name |node_name| and properties
  // |node_properties|. The child node's node controller in `metadata_node_controllers_`.
  zx_status_t AddChildNode(
      std::string_view node_name,
      const std::vector<fuchsia_driver_framework::NodeProperty2>& node_properties);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  fdf_metadata::MetadataServer<fuchsia_hardware_test::Metadata> metadata_server_;
  bool offer_metadata_to_child_nodes_ = false;
#endif

  std::vector<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> metadata_recipients_;
  fidl::ServerBindingGroup<fuchsia_hardware_test::MetadataSender> bindings_;
};

}  // namespace fdf_metadata::test

#endif  // LIB_DRIVER_METADATA_CPP_TESTS_METADATA_INTEGRATION_TEST_METADATA_SENDER_TEST_DRIVER_METADATA_SENDER_TEST_DRIVER_H_
