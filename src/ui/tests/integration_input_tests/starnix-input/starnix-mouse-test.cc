// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include "relay-api.h"
#include "src/ui/testing/util/portable_ui_test.h"
#include "src/ui/tests/integration_input_tests/starnix-input/starnix-input-test-base.h"
#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/input-event-codes.h"

struct MouseEvent {
  int scroll_v;  // The number of ticks scrolled up/down.
};

class StarnixMouseTest : public starnix_input_test::StarnixInputTestBase {
 protected:
  // To satisfy ::testing::Test
  void SetUp() override {
    ui_testing::PortableUITest::SetUp();
    FX_LOGS(INFO) << "Registering input injection device";
    RegisterMouse();
  }

  // Reads sequences of mouse events from `input_dump.cc`, via `out_socket`
  // until we get num_expected events.
  //
  // Because of the variable amount of packets read at a time, we may create
  // varying  amounts of MouseEvents from a single call to GetEvDevPackets.
  // Therefore we use the running size of the final result to determine whether
  // to read more packets.
  std::vector<MouseEvent> GetMouseEventSequenceOfLen(zx::socket& out_socket, size_t num_expected) {
    std::vector<MouseEvent> result;

    while (result.size() < num_expected) {
      std::vector<EvDevPacket> pkts = GetEvDevPackets(out_socket);

      for (EvDevPacket pkt : pkts) {
        if (pkt.type == EV_SYN) {
          continue;
        }

        if (pkt.type != EV_REL) {
          FX_LOGS(FATAL) << "unexpected event type in mouse event seq, type=" << pkt.type;
        }

        if (pkt.code != REL_WHEEL) {
          FX_LOGS(FATAL) << "unexpected event code in mouse event seq, code=" << pkt.code;
        }
        result.push_back(MouseEvent{.scroll_v = pkt.value});
      }
      FX_LOGS(INFO) << "Read " << result.size() << " events of " << num_expected;
    }

    EXPECT_EQ(result.size(), num_expected);
    return result;
  }

  void InitMouseInputRelay(zx::socket& in_socket, zx::socket& out_socket) {
    // Wait for `input_dump` to start.
    WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);
    // Inform `input_dump` to run the input_relay.
    std::stringstream ss;
    ss << relay_api::kDeviceDelimiter << " " << relay_api::kMouseDev;
    WriteMessageToSocket(in_socket, ss.str());
  }
};

TEST_F(StarnixMouseTest, Scroll) {
  auto [in_socket, out_socket] = LaunchDumper();

  // Wait until #launch_input is presented before injecting input.
  WaitForViewPresentation();

  // Start `input_dump` for a mouse device.
  InitMouseInputRelay(in_socket, out_socket);

  // `input_dump` is ready to receive events.
  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  std::stringstream ss;
  // This test expects 3 scroll event sequences.
  ss << relay_api::kEventCmd << " " << relay_api::kScrollNumPackets * 3;
  WriteMessageToSocket(in_socket, ss.str());

  // Wait for `input_dump` to be ready for event injection.
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Empty event that wakes container.
  // TODO(b/438244012): Starnix input relay doubles the first event received from InputPipeline
  // upon container wakeup.
  SimulateMouseScroll({}, 0, 0);

  // Scroll down 1 tick.
  SimulateMouseScroll({}, 0, -1);
  {
    auto events = GetMouseEventSequenceOfLen(out_socket, 1);
    EXPECT_EQ(events[0].scroll_v, -1);
  }

  // Scroll down 4 ticks.
  SimulateMouseScroll({}, 0, -4);
  {
    auto events = GetMouseEventSequenceOfLen(out_socket, 1);
    EXPECT_EQ(events[0].scroll_v, -4);
  }

  // Scroll back up.
  SimulateMouseScroll({}, 0, 5);
  {
    auto events = GetMouseEventSequenceOfLen(out_socket, 1);
    EXPECT_EQ(events[0].scroll_v, 5);
  }

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, "quit");
}

// EventsDuringFileCloseAreIgnored ensure event reader does not get events before open.
TEST_F(StarnixMouseTest, EventsDuringFileCloseAreIgnored) {
  auto [in_socket, out_socket] = LaunchDumper();

  // Wait until #launch_input is presented before injecting input.
  WaitForViewPresentation();

  // Start `input_dump` for a mouse device.
  InitMouseInputRelay(in_socket, out_socket);

  // `input_dump` is ready to receive events.
  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  std::stringstream ss;
  ss << relay_api::kEventCmd << " " << relay_api::kScrollNumPackets;
  WriteMessageToSocket(in_socket, ss.str());
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Empty event that wakes container.
  // TODO(b/438244012): Starnix input relay doubles the first event received from InputPipeline
  // upon container wakeup.
  SimulateMouseScroll({}, 0, 0);

  // Scroll down 1 tick.
  SimulateMouseScroll({}, 0, -1);
  {
    auto events = GetMouseEventSequenceOfLen(out_socket, 1);
    EXPECT_EQ(events[0].scroll_v, -1);
  }

  FX_LOGS(INFO) << "device file closed";

  // Now the file is closed. Send 1 scroll event. input_dump should not receive this event
  // sequence.
  SimulateMouseScroll({}, 0, -1);

  // TODO(b/375021518): Here should block on a state instead of timeout to ensure events are
  // reached to Starnix before open file below.
  // It is ok if this test is flaky when the tap top left reach to starnix after "device file
  // opened". It just means this timeout is not enough.
  RunLoopWithTimeout(zx::sec(1));

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, ss.str());

  FX_LOGS(INFO) << "device file opened";

  // Wait for `input_dump` to ready for event injection.
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Send 1 scroll up event. input_dump should receive this event sequence.
  SimulateMouseScroll({}, 0, 1);
  {
    auto events = GetMouseEventSequenceOfLen(out_socket, 1);
    EXPECT_EQ(events[0].scroll_v, 1);
  }

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, "quit");
}
