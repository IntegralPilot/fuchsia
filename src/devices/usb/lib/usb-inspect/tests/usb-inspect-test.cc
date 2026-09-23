// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <usb-inspect/usb-inspect.h>

namespace usb_inspect {

using namespace inspect::testing;

class UsbInspectTest : public ::testing::Test {
 public:
  inspect::Hierarchy ReadInspect(const inspect::Inspector& inspector) {
    fpromise::result<inspect::Hierarchy> result =
        fpromise::run_single_threaded(inspect::ReadFromInspector(inspector));
    EXPECT_TRUE(result.is_ok());
    return std::move(result.value());
  }
};

TEST_F(UsbInspectTest, TestEndpointInspect) {
  inspect::Inspector inspector;
  EndpointInspect endpoint;

  endpoint.Init(inspector.GetRoot(), "endpoint_test", 3);

  endpoint.UpdateTxQueue(10);
  endpoint.UpdateRxQueue(20);
  endpoint.UpdateRxPendingProcessing(30);
  endpoint.AddTxBytes(1024);
  endpoint.AddRxBytes(2048);

  // Trigger throughput calculation (Total bytes = 1024 Tx + 2048 Rx = 3072 bytes over 1 second)
  endpoint.MeasureThroughput(zx::sec(1));

  endpoint.RecordEvent("event_1");
  endpoint.RecordEvent("event_2");
  endpoint.RecordEvent("event_3");
  endpoint.RecordEvent("event_4");  // should overwrite event_1 (modulo capacity 3)

  auto hierarchy = ReadInspect(inspector);

  auto* endpoint_node = hierarchy.GetByPath({"endpoint_test"});
  ASSERT_THAT(endpoint_node, ::testing::NotNull());

  // Assert basic properties
  EXPECT_THAT(endpoint_node->node(),
              PropertyList(::testing::UnorderedElementsAre(
                  UintIs("tx_pending_requests", 10), UintIs("rx_pending_requests", 20),
                  UintIs("rx_pending_processing", 30), UintIs("total_bytes_tx", 1024),
                  UintIs("total_bytes_rx", 2048), UintIs("max_bytes_per_second", 3072))));

  // Assert event history circular list size is exactly capacity 3
  auto* history_node = hierarchy.GetByPath({"endpoint_test", "event_history"});
  ASSERT_THAT(history_node, ::testing::NotNull());
  EXPECT_EQ(3u, history_node->children().size());
}

TEST_F(UsbInspectTest, TestDciInspect) {
  inspect::Inspector inspector;
  DciInspect dci;

  dci.Init(inspector.GetRoot(), "dci_test");
  dci.UpdateState("kPeripheralReady");
  dci.UpdateConnectionStatus(true, USB_SPEED_SUPER);
  dci.UpdateUsbMode(USB_MODE_PERIPHERAL);

  auto hierarchy = ReadInspect(inspector);

  auto* dci_node = hierarchy.GetByPath({"dci_test"});
  ASSERT_THAT(dci_node, ::testing::NotNull());

  EXPECT_THAT(dci_node->node(),
              PropertyList(::testing::UnorderedElementsAre(
                  StringIs("state", "kPeripheralReady"), BoolIs("connected", true),
                  StringIs("speed", "super"), StringIs("usb_mode", "PERIPHERAL"))));
}

TEST_F(UsbInspectTest, TestFunctionInspect) {
  inspect::Inspector inspector;
  FunctionInspect func;

  func.Init(inspector.GetRoot(), "function_test", 2);
  func.UpdateConfiguration(1, true);
  func.UpdateDescriptorInfo(255, 66, 1);

  auto hierarchy = ReadInspect(inspector);

  auto* func_node = hierarchy.GetByPath({"function_test"});
  ASSERT_THAT(func_node, ::testing::NotNull());

  EXPECT_THAT(func_node->node(),
              PropertyList(::testing::UnorderedElementsAre(
                  UintIs("index", 2), UintIs("configuration", 1), BoolIs("configured", true),
                  UintIs("interface_class", 255), UintIs("interface_subclass", 66),
                  UintIs("interface_protocol", 1))));
}

TEST_F(UsbInspectTest, TestDciInspectHistory) {
  inspect::Inspector inspector;
  DciInspect dci;

  // Initialize with control capacity 2 and connection capacity 2 to easily test circular buffer
  // wrap-around
  dci.Init(inspector.GetRoot(), "dci_test", 2);

  // 1. Verify General Event History
  dci.RecordEvent("dci_event_1");
  dci.RecordEvent("dci_event_2");

  // 2. Verify Control Transfer Circular History
  // standard GET_DESCRIPTOR
  dci.RecordControlTransfer({
      .request_type = 0x80,
      .request = 0x06,
      .value = 0x0100,
      .index = 0x0000,
      .length = 18,
      .status = ZX_OK,
      .actual_length = 18,
  });
  // standard SET_CONFIGURATION
  dci.RecordControlTransfer({
      .request_type = 0x00,
      .request = 0x09,
      .value = 0x0001,
      .index = 0x0000,
      .length = 0,
      .status = ZX_OK,
      .actual_length = 0,
  });
  // vendor request that failed
  dci.RecordControlTransfer({
      .request_type = 0x40,
      .request = 0x0A,
      .value = 0x1234,
      .index = 0x5678,
      .length = 8,
      .status = ZX_ERR_IO_REFUSED,
      .actual_length = 0,
  });  // Should overwrite the first entry

  auto hierarchy = ReadInspect(inspector);

  // Check general events
  auto* event_history = hierarchy.GetByPath({"dci_test", "event_history"});
  ASSERT_THAT(event_history, ::testing::NotNull());
  EXPECT_EQ(2u, event_history->children().size());

  // Check control transfers
  auto* control_history = hierarchy.GetByPath({"dci_test", "control_history"});
  ASSERT_THAT(control_history, ::testing::NotNull());
  EXPECT_EQ(2u, control_history->children().size());  // Exactly capacity 2

  // Verify the active items in circular list. Since idx 2 overwrote idx 0:
  // slot "1" should be set to standard SET_CONFIGURATION
  // slot "2" should be vendor request that failed
  auto* slot_1 = hierarchy.GetByPath({"dci_test", "control_history", "1"});
  auto* slot_2 = hierarchy.GetByPath({"dci_test", "control_history", "2"});
  ASSERT_THAT(slot_1, ::testing::NotNull());
  ASSERT_THAT(slot_2, ::testing::NotNull());

  EXPECT_THAT(slot_2->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("bm_request_type", 0x40), UintIs("b_request", 0x0A),
                                  UintIs("w_value", 0x1234), UintIs("w_index", 0x5678),
                                  UintIs("w_length", 8), IntIs("status", ZX_ERR_IO_REFUSED),
                                  UintIs("response_length", 0), UintIs("@time", ::testing::_))));

  EXPECT_THAT(slot_1->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("bm_request_type", 0x00), UintIs("b_request", 0x09),
                                  UintIs("w_value", 0x0001), UintIs("w_index", 0x0000),
                                  UintIs("w_length", 0), IntIs("status", ZX_OK),
                                  UintIs("response_length", 0), UintIs("@time", ::testing::_))));
}

TEST_F(UsbInspectTest, TestEventHistoryTrafficSnapshots) {
  inspect::Inspector inspector;
  EndpointInspect endpoint;

  endpoint.Init(inspector.GetRoot(), "endpoint_test", 5);

  endpoint.RecordEvent("state_changed: kStoppingUsb");
  endpoint.AddTxBytes(1024);
  endpoint.AddRxBytes(2048);
  endpoint.MeasureThroughput(zx::sec(1));

  endpoint.RecordEvent("state_changed: kOnline");
  endpoint.AddTxBytes(4096);

  auto hierarchy = ReadInspect(inspector);

  auto* snap_0 = hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "0"});
  ASSERT_THAT(snap_0, ::testing::NotNull());
  EXPECT_THAT(snap_0->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("@time", ::testing::_), UintIs("total_bytes_tx", 0),
                                  UintIs("total_bytes_rx", 0), UintIs("max_bytes_per_second", 0))));

  auto* snap_1 = hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "1"});
  ASSERT_THAT(snap_1, ::testing::NotNull());
  EXPECT_THAT(snap_1->node(),
              PropertyList(::testing::UnorderedElementsAre(
                  UintIs("@time", ::testing::_), UintIs("total_bytes_tx", 1024),
                  UintIs("total_bytes_rx", 2048), UintIs("max_bytes_per_second", 3072))));
}

TEST_F(UsbInspectTest, TestEventHistoryTrafficSnapshotsEviction) {
  inspect::Inspector inspector;
  EndpointInspect endpoint;

  endpoint.Init(inspector.GetRoot(), "endpoint_test", 2, 2);

  endpoint.RecordEvent("event_0");
  endpoint.AddTxBytes(100);
  endpoint.RecordEvent("event_1");
  endpoint.AddTxBytes(200);
  endpoint.RecordEvent("event_2");  // Evicts event_0
  endpoint.AddTxBytes(300);

  auto hierarchy = ReadInspect(inspector);
  EXPECT_THAT(hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "0"}),
              ::testing::IsNull());
  auto* snap_1 = hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "1"});
  ASSERT_THAT(snap_1, ::testing::NotNull());
  EXPECT_THAT(snap_1->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("@time", ::testing::_), UintIs("total_bytes_tx", 100),
                                  UintIs("total_bytes_rx", 0), UintIs("max_bytes_per_second", 0))));

  auto* snap_2 = hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "2"});
  ASSERT_THAT(snap_2, ::testing::NotNull());
  EXPECT_THAT(snap_2->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("@time", ::testing::_), UintIs("total_bytes_tx", 300),
                                  UintIs("total_bytes_rx", 0), UintIs("max_bytes_per_second", 0))));
}

TEST_F(UsbInspectTest, TestDirectSnapshotTransferStats) {
  inspect::Inspector inspector;
  EndpointInspect endpoint;

  endpoint.Init(inspector.GetRoot(), "endpoint_test", 5);

  endpoint.AddTxBytes(500);
  endpoint.SnapshotTransferStats();

  endpoint.AddTxBytes(250);
  endpoint.SnapshotTransferStats();

  auto hierarchy = ReadInspect(inspector);
  auto* snap_0 = hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "0"});
  ASSERT_THAT(snap_0, ::testing::NotNull());
  EXPECT_THAT(snap_0->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("@time", ::testing::_), UintIs("total_bytes_tx", 500),
                                  UintIs("total_bytes_rx", 0), UintIs("max_bytes_per_second", 0))));

  auto* snap_1 = hierarchy.GetByPath({"endpoint_test", "transfer_snapshots", "1"});
  ASSERT_THAT(snap_1, ::testing::NotNull());
  EXPECT_THAT(snap_1->node(), PropertyList(::testing::UnorderedElementsAre(
                                  UintIs("@time", ::testing::_), UintIs("total_bytes_tx", 750),
                                  UintIs("total_bytes_rx", 0), UintIs("max_bytes_per_second", 0))));
}

TEST_F(UsbInspectTest, TestZeroCapacity) {
  inspect::Inspector inspector;
  EndpointInspect endpoint;

  endpoint.Init(inspector.GetRoot(), "endpoint_test", 0, 0);
  endpoint.RecordEvent("event_0");
  endpoint.SnapshotTransferStats();

  auto hierarchy = ReadInspect(inspector);
  EXPECT_THAT(hierarchy.GetByPath({"endpoint_test", "event_history"}), ::testing::IsNull());
  EXPECT_THAT(hierarchy.GetByPath({"endpoint_test", "transfer_snapshots"}), ::testing::IsNull());
}

}  // namespace usb_inspect
