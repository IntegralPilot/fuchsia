// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_METADATA_CPP_TESTS_METADATA_INTEGRATION_TEST_METADATA_RETRIEVER_TEST_DRIVER_METADATA_RETRIEVER_TEST_DRIVER_H_
#define LIB_DRIVER_METADATA_CPP_TESTS_METADATA_INTEGRATION_TEST_METADATA_RETRIEVER_TEST_DRIVER_METADATA_RETRIEVER_TEST_DRIVER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/node/cpp/add_child.h>

namespace fdf_metadata::test {

// This driver's purpose is to try to retrieve metadata from its parent node using
// `fdf::GetMetadata()`.
class MetadataRetrieverTestDriver : public fdf::DriverBase2,
                                    public fidl::Server<fuchsia_hardware_test::MetadataRetriever> {
 public:
  static constexpr std::string_view kDriverName = "retriever";

  MetadataRetrieverTestDriver() : DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;

  // fidl::Server<fuchsia_hardware_test::MetadataRetriever> implementation.
  void GetMetadata(GetMetadataCompleter::Sync& completer) override;
  void GetMetadataIfExists(GetMetadataIfExistsCompleter::Sync& completer) override;

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_test::MetadataRetriever> bindings_;
  std::unique_ptr<fdf::Namespace> incoming_;
};

}  // namespace fdf_metadata::test

#endif  // LIB_DRIVER_METADATA_CPP_TESTS_METADATA_INTEGRATION_TEST_METADATA_RETRIEVER_TEST_DRIVER_METADATA_RETRIEVER_TEST_DRIVER_H_
