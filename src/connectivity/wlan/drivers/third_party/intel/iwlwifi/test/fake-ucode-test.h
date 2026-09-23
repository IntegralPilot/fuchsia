// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_TEST_FAKE_UCODE_TEST_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_TEST_FAKE_UCODE_TEST_H_

#include <memory>
#include <vector>

#include <gtest/gtest.h>

#include "third_party/iwlwifi/fw/file.h"
#include "third_party/iwlwifi/test/sim-trans.h"

namespace wlan::testing {

// Helper class for unit test code to inherit in order to create an fake firmware with
// customized ucode capability.
//
// Note that the contructor will call the init function of transportation layer,
// and assert it is successful. The test case doesn't need to init again.
//
class FakeUcodeTest : public ::testing::Test {
 public:
  // Creates a fake firmware with the given capabilities/apis.
  FakeUcodeTest(const std::vector<enum iwl_ucode_tlv_capa>& capas,
                const std::vector<enum iwl_ucode_tlv_api>& apis);
  ~FakeUcodeTest() = default;

 protected:
  SimTransport sim_trans_;
};

}  // namespace wlan::testing

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_TEST_FAKE_UCODE_TEST_H_
