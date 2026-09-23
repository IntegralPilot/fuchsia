// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_TESTING_BOARD_TEST_HELPER_H_
#define LIB_DRIVER_DEVICETREE_TESTING_BOARD_TEST_HELPER_H_

#include <lib/async/dispatcher.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zbi-format/board.h>
#include <lib/zx/result.h>

#include <string>

namespace fdf_devicetree::testing {

// Test helper class for devicetree board driver integration testing
//
// Creates a test realm with driver framework, platform bus and the board
// driver components running in it. The test realm creates a platform bus
// device with specified VID, PID provided in the |platform_id| to which the
// board driver will bind to. The |dtb_path| file is provided to the board
// driver through the fake |fuchsia_boot::Items| protocol implemented by this library.
//
// See sdk/lib/driver/devicetree/examples/example-board/integration-test.cc for usage.
//
class BoardTestHelper {
 public:
  explicit BoardTestHelper(std::string dtb_path, zbi_platform_id_t platform_id,
                           async_dispatcher_t* dispatcher, bool dtr_v2 = false)
      : dtb_path_(std::move(dtb_path)),
        platform_id_(platform_id),
        dispatcher_(dispatcher),
        dtr_v2_(dtr_v2) {}

  ~BoardTestHelper();

  // Setup fuchsia_boot mock and driver test realm.
  void SetupRealm();

  // Start the realm and driver test realm.
  zx::result<> StartRealm();

  // Helper method for tests to enumerate devices.
  zx::result<> WaitOnDevices(const std::vector<std::string>& device_paths);

  component_testing::RealmRoot* realm() { return realm_.get(); }
  component_testing::RealmBuilder* realm_builder() { return realm_builder_.get(); }

 private:
  std::unique_ptr<component_testing::RealmBuilder> realm_builder_;
  std::unique_ptr<component_testing::RealmRoot> realm_;
  const std::string dtb_path_;
  const zbi_platform_id_t platform_id_;
  async_dispatcher_t* dispatcher_;
  bool dtr_v2_;
};

}  // namespace fdf_devicetree::testing

#endif  // LIB_DRIVER_DEVICETREE_TESTING_BOARD_TEST_HELPER_H_
