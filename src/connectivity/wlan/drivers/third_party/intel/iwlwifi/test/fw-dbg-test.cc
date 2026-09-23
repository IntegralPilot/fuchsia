// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/inspect/cpp/reader.h>

#include <algorithm>
#include <memory>

#include <gtest/gtest.h>

extern "C" {
#include "third_party/iwlwifi/fw/dbg.h"
#include "third_party/iwlwifi/fw/error-dump.h"
#include "third_party/iwlwifi/fw/runtime.h"
}

#include "third_party/iwlwifi/platform/driver-inspector.h"
#include "third_party/iwlwifi/test/single-ap-test.h"

namespace wlan::testing {
namespace {

class FwDbgTest : public SingleApTest {
 public:
  FwDbgTest();
  ~FwDbgTest() override;

 protected:
  std::unique_ptr<::wlan::iwlwifi::DriverInspector> inspector_;
  struct iwl_fw fw_ = {};
  struct iwl_fw_runtime fwrt_ = {};
  component::OutgoingDirectory outgoing_;
};

FwDbgTest::FwDbgTest() : outgoing_(fdf::Dispatcher::GetCurrent()->async_dispatcher()) {
  inspector_ = std::make_unique<::wlan::iwlwifi::DriverInspector>(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing_,
      wlan::iwlwifi::DriverInspectorOptions{.root_name = "fw_dbg_test"});
  sim_trans_.iwl_trans()->dev->inspector = static_cast<struct driver_inspector*>(inspector_.get());
  iwl_fw_runtime_init(&fwrt_, sim_trans_.iwl_trans(), &fw_, nullptr, nullptr, nullptr);
}

FwDbgTest::~FwDbgTest() { iwl_fw_runtime_free(&fwrt_); }

// Test a user-triggered firmware debug dump.
TEST_F(FwDbgTest, TestUserTrigger) {
  static const char kFwErrorDumpName[] = "fw_dbg_test_dump";
  iwl_fw_dbg_collect(&fwrt_, FW_DBG_TRIGGER_USER, kFwErrorDumpName, sizeof(kFwErrorDumpName));
  iwl_fw_flush_dump(&fwrt_);

  // Check that the dump exists.
  auto root_hierarchy = ::inspect::ReadFromVmo(inspector_->DuplicateVmo()).take_value();
  EXPECT_EQ(1, root_hierarchy.children().size());
  auto hierarchy = root_hierarchy.GetByPath({"fw_dbg_test"});
  EXPECT_NE(nullptr, hierarchy);

  if (hierarchy != nullptr) {
    EXPECT_EQ(1, hierarchy->node().properties().size());
    auto prop = hierarchy->node().get_property<inspect::ByteVectorPropertyValue>(kFwErrorDumpName);
    EXPECT_NE(nullptr, prop);

    // Sanity check the header of the dump.
    if (prop != nullptr) {
      const uint32_t barker = cpu_to_le32(IWL_FW_ERROR_DUMP_BARKER);
      EXPECT_LE(sizeof(barker), prop->value().size());
      EXPECT_EQ(0, std::memcmp(&barker, prop->value().data(),
                               std::min(sizeof(barker), prop->value().size())));
    }
  }
}

}  // namespace
}  // namespace wlan::testing
