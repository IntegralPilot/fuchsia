// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.app/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/natural_ostream.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <lib/stdcompat/source_location.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <sstream>
#include <vector>

#include <gtest/gtest.h>

#include "relay-api.h"
#include "src/ui/testing/util/portable_ui_test.h"
#include "src/ui/tests/integration_input_tests/starnix-input/starnix-input-test-base.h"
#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/input-event-codes.h"

namespace {

// Maximum distance between two physical pixel coordinates so that they are considered equal.
constexpr double kEpsilon = 0.5f;

// Touch down is expressed in two `TouchEvents`s: btn_touch, Phase Add.
// Touch up is expressed in two: btn_touch, Phase Remove.
constexpr size_t kDownUpNumEvents = 4;

// Touch move is expressed in one `TouchEvents`: Phase Change.
// Touch up is expressed in two: btn_touch, Phase Remove.
constexpr size_t kMoveUpNumEvents = 3;

struct TouchEvent {
  float local_x;  // The x-position, in the client's coordinate space.
  float local_y;  // The y-position, in the client's coordinate space.
  fuchsia_ui_pointer::EventPhase phase;
  int slot_id;
  int pointer_id;      // only phase add include this field.
  bool has_btn_touch;  // only btn_touch event will only have this field.
  int btn_touch;       // only btn_touch event will only have this field.
};

void ExpectLocationPhaseAndSlot(
    const TouchEvent& e, double expected_x, double expected_y,
    fuchsia_ui_pointer::EventPhase expected_phase, int expected_slot_id,
    const cpp20::source_location caller = cpp20::source_location::current()) {
  std::string caller_info = "line " + std::to_string(caller.line());
  EXPECT_EQ(expected_slot_id, e.slot_id) << " from " << caller_info;
  EXPECT_EQ(expected_phase, e.phase) << " from " << caller_info;
  EXPECT_NEAR(expected_x, e.local_x, kEpsilon) << " from " << caller_info;
  EXPECT_NEAR(expected_y, e.local_y, kEpsilon) << " from " << caller_info;
  EXPECT_EQ(false, e.has_btn_touch) << " from " << caller_info;
}

void ExpectBtnTouch(const TouchEvent& e, int expected_value,
                    const cpp20::source_location caller = cpp20::source_location::current()) {
  std::string caller_info = "line " + std::to_string(caller.line());
  EXPECT_EQ(true, e.has_btn_touch) << " from " << caller_info;
  EXPECT_EQ(expected_value, e.btn_touch) << " from " << caller_info;
}

enum class TapLocation { kTopLeft, kBottomRight };

class StarnixTouchTest : public starnix_input_test::StarnixInputTestBase {
 protected:
  ~StarnixTouchTest() override {
    FX_CHECK(touch_injection_request_count() > 0) << "injection expected but didn't happen.";
  }

  // To satisfy ::testing::Test
  void SetUp() override {
    ui_testing::PortableUITest::SetUp();
    FX_LOGS(INFO) << "Registering input injection device";
    RegisterTouchScreen();
  }

  // For use by test cases.
  void InjectInput(TapLocation tap_location) {
    switch (tap_location) {
      case TapLocation::kTopLeft:
        InjectTap(display_width() / 4, display_height() / 4);
        break;
      case TapLocation::kBottomRight:
        InjectTap(3 * display_width() / 4, 3 * display_height() / 4);
        break;
    }
  }

  // Reads sequences of touch events from `input_dump.cc`, via `out_socket`
  // until we get num_expected events.
  //
  // Because of the variable amount of packets read at a time, we may create
  // varying  amounts of TouchEvents from a single call to GetEvDevPackets.
  // Therefore we use the running size of the final result to determine whether
  // to read more packets.
  std::vector<TouchEvent> GetTouchEventSequenceOfLen(zx::socket& out_socket, size_t num_expected) {
    std::vector<TouchEvent> result;

    while (result.size() < num_expected) {
      std::vector<EvDevPacket> pkts = GetEvDevPackets(out_socket);

      for (EvDevPacket pkt : pkts) {
        if (pkt.type == EV_SYN) {
          continue;
        }

        if (pkt.type == EV_KEY) {
          if (pkt.code != BTN_TOUCH) {
            FX_LOGS(FATAL) << "unexpected key event code in touch event seq, code=" << pkt.code;
          }
          result.push_back(TouchEvent{.has_btn_touch = true, .btn_touch = pkt.value});

          continue;
        }

        if (pkt.type != EV_ABS) {
          FX_LOGS(FATAL) << "unexpected event type in touch event seq, type=" << pkt.type;
        }

        switch (pkt.code) {
          case ABS_MT_SLOT:
            result.push_back(
                TouchEvent{.phase = fuchsia_ui_pointer::EventPhase::kChange, .slot_id = pkt.value});
            break;
          case ABS_MT_TRACKING_ID:
            if (result.empty()) {
              FX_LOGS(FATAL) << "receive ABS_MT_TRACKING_ID out of slot";
            }

            if (pkt.value == -1) {
              result[result.size() - 1].phase = fuchsia_ui_pointer::EventPhase::kRemove;
            } else {
              result[result.size() - 1].phase = fuchsia_ui_pointer::EventPhase::kAdd;
              result[result.size() - 1].pointer_id = pkt.value;
            }

            break;
          case ABS_MT_POSITION_X:
            if (result.empty()) {
              FX_LOGS(FATAL) << "receive ABS_MT_POSITION_X out of slot";
            }

            result[result.size() - 1].local_x = static_cast<float>(pkt.value);

            break;
          case ABS_MT_POSITION_Y:
            if (result.empty()) {
              FX_LOGS(FATAL) << "receive ABS_MT_POSITION_X out of slot";
            }

            result[result.size() - 1].local_y = static_cast<float>(pkt.value);

            break;
          default:
            FX_LOGS(FATAL) << "unexpected event code in touch event seq, code=" << pkt.code;
        }
      }
      FX_LOGS(INFO) << "Read " << result.size() << " events of " << num_expected;
    }

    EXPECT_EQ(result.size(), num_expected);
    return result;
  }

  void InitTouchInputRelay(zx::socket& in_socket, zx::socket& out_socket) {
    // Wait for `input_dump` to start.
    WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);
    // Inform `input_dump` to run the touch_relay.
    std::stringstream ss;
    ss << relay_api::kDeviceDelimiter << " " << relay_api::kTouchDev;
    WriteMessageToSocket(in_socket, ss.str());
  }
};

// TODO: https://fxbug.dev/42082519 - Test for DPR=2.0, too.
TEST_F(StarnixTouchTest, Tap) {
  auto [in_socket, out_socket] = LaunchDumper();

  // Wait until #launch_input is presented before injecting input.
  WaitForViewPresentation();

  // Start `input_dump` for a touch device.
  InitTouchInputRelay(in_socket, out_socket);

  // `input_dump` is ready to receive events.
  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  std::stringstream ss;
  // This test expects 2 down - up event sequences.
  ss << relay_api::kEventCmd << " " << relay_api::kDownUpNumPackets * 2;
  WriteMessageToSocket(in_socket, ss.str());

  // Wait for `input_dump` to ready for event injection.
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Top-left.
  InjectInput(TapLocation::kTopLeft);

  {
    auto events = GetTouchEventSequenceOfLen(out_socket, kDownUpNumEvents);
    ExpectLocationPhaseAndSlot(events[0], static_cast<float>(display_width()) / 4.f,
                               static_cast<float>(display_height()) / 4.f,
                               fuchsia_ui_pointer::EventPhase::kAdd, 0);
    ExpectBtnTouch(events[1], 1);
    ExpectLocationPhaseAndSlot(events[2], 0.0, 0.0, fuchsia_ui_pointer::EventPhase::kRemove, 0);
    ExpectBtnTouch(events[3], 0);
  }

  // Bottom-right.
  InjectInput(TapLocation::kBottomRight);

  {
    auto events = GetTouchEventSequenceOfLen(out_socket, kDownUpNumEvents);
    ExpectLocationPhaseAndSlot(events[0], 3 * static_cast<float>(display_width()) / 4.f,
                               3 * static_cast<float>(display_height()) / 4.f,
                               fuchsia_ui_pointer::EventPhase::kAdd, 0);
    ExpectBtnTouch(events[1], 1);
    ExpectLocationPhaseAndSlot(events[2], 0.0, 0.0, fuchsia_ui_pointer::EventPhase::kRemove, 0);
    ExpectBtnTouch(events[3], 0);
  }

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, "quit");
}

// EventsDuringFileCloseAreIgnored ensure event reader does not get events before open.
TEST_F(StarnixTouchTest, EventsDuringFileCloseAreIgnored) {
  auto [in_socket, out_socket] = LaunchDumper();

  // Wait until #launch_input is presented before injecting input.
  WaitForViewPresentation();

  // Start `input_dump` for a touch device.
  InitTouchInputRelay(in_socket, out_socket);

  // `input_dump` is ready to receive events.
  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  std::stringstream ss;
  ss << relay_api::kEventCmd << " " << relay_api::kDownUpNumPackets;
  WriteMessageToSocket(in_socket, ss.str());
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Send 1 tap to top left.
  InjectInput(TapLocation::kTopLeft);
  {
    auto events = GetTouchEventSequenceOfLen(out_socket, kDownUpNumEvents);
    ExpectLocationPhaseAndSlot(events[0], static_cast<float>(display_width()) / 4.f,
                               static_cast<float>(display_height()) / 4.f,
                               fuchsia_ui_pointer::EventPhase::kAdd, 0);
    ExpectBtnTouch(events[1], 1);
    ExpectLocationPhaseAndSlot(events[2], 0.0, 0.0, fuchsia_ui_pointer::EventPhase::kRemove, 0);
    ExpectBtnTouch(events[3], 0);
  }

  FX_LOGS(INFO) << "device file closed";

  // Now the file is closed. Send 1 tap to top left. input_dump should not receive this event
  // sequence.
  InjectInput(TapLocation::kTopLeft);

  // TODO(https://fxbug.dev/375021518): Here should block on a state instead of timeout to ensure
  // events are reached to Starnix before open file below.
  // It is ok if this test is flaky when the tap top left reach to starnix after "device file
  // opened". It just means this timeout is not enough.
  RunLoopWithTimeout(zx::sec(1));

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, ss.str());

  FX_LOGS(INFO) << "device file opened";

  // Wait for `input_dump` to ready for event injection.
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Send 1 tap to bottom right. input_dump should receive this event sequence.
  InjectInput(TapLocation::kBottomRight);
  {
    auto events = GetTouchEventSequenceOfLen(out_socket, kDownUpNumEvents);
    ExpectLocationPhaseAndSlot(events[0], 3 * static_cast<float>(display_width()) / 4.f,
                               3 * static_cast<float>(display_height()) / 4.f,
                               fuchsia_ui_pointer::EventPhase::kAdd, 0);
    ExpectBtnTouch(events[1], 1);
    ExpectLocationPhaseAndSlot(events[2], 0.0, 0.0, fuchsia_ui_pointer::EventPhase::kRemove, 0);
    ExpectBtnTouch(events[3], 0);
  }

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, "quit");
}

// OpenFileDuringEventSequenceReceivesPartialSequence tests event delivery when the device file is
// opened in the middle of an event sequence. It verifies that only events generated after the file
// is opened are recorded in input_dump.
TEST_F(StarnixTouchTest, OpenFileDuringEventSequenceReceivesPartialSequence) {
  auto [in_socket, out_socket] = LaunchDumper();

  // Wait until #launch_input is presented before injecting input.
  WaitForViewPresentation();

  // Start `input_dump` for a touch device.
  InitTouchInputRelay(in_socket, out_socket);

  // `input_dump` is ready to receive events.
  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  std::stringstream ss;
  ss << relay_api::kEventCmd << " " << relay_api::kDownUpNumPackets;
  WriteMessageToSocket(in_socket, ss.str());
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  // Send 1 tap to top left.
  InjectInput(TapLocation::kTopLeft);
  {
    auto events = GetTouchEventSequenceOfLen(out_socket, kDownUpNumEvents);
    ExpectLocationPhaseAndSlot(events[0], static_cast<float>(display_width()) / 4.f,
                               static_cast<float>(display_height()) / 4.f,
                               fuchsia_ui_pointer::EventPhase::kAdd, 0);
    ExpectBtnTouch(events[1], 1);
    ExpectLocationPhaseAndSlot(events[2], 0.0, 0.0, fuchsia_ui_pointer::EventPhase::kRemove, 0);
    ExpectBtnTouch(events[3], 0);
  }

  FX_LOGS(INFO) << "device file closed";

  // Now the file is closed. Send 1 tap to top left. input_dump should not receive this down event.
  fuchsia_ui_test_input::TouchInputReport down;
  down.contacts({{fuchsia_ui_test_input::ContactInputReport{
      {
          .contact_id = 1,
          .position_x = display_width() / 4,
          .position_y = display_height() / 4,
      },
  }}});
  InjectTouchEvent(down);

  // TODO(https://fxbug.dev/375021518): Here should block on a state instead of timeout to ensure
  // events are reached to Starnix before open file below.
  // It is ok if this test is flaky when the tap top left reach to starnix after "device file
  // opened". It just means this timeout is not enough.
  RunLoopWithTimeout(zx::sec(1));

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  // Inject touch move and up.
  ss.str("");  // clear the stringstream
  ss << relay_api::kEventCmd << " " << relay_api::kMoveNumPackets + relay_api::kUpNumPackets;
  WriteMessageToSocket(in_socket, ss.str());

  FX_LOGS(INFO) << "device file opened";

  // Wait for `input_dump` to ready for event injection.
  WaitForMessageFromInputDump(out_socket, relay_api::kReadyMessage);

  fuchsia_ui_test_input::TouchInputReport move;
  move.contacts({{fuchsia_ui_test_input::ContactInputReport{
      {
          .contact_id = 1,
          .position_x = display_width() / 4 * 3,
          .position_y = display_height() / 4 * 3,
      },
  }}});
  InjectTouchEvent(move);

  fuchsia_ui_test_input::TouchInputReport up;
  up.contacts({{}});
  InjectTouchEvent(up);

  {
    auto events = GetTouchEventSequenceOfLen(out_socket, kMoveUpNumEvents);
    ExpectLocationPhaseAndSlot(events[0], 3 * static_cast<float>(display_width()) / 4.f,
                               3 * static_cast<float>(display_height()) / 4.f,
                               fuchsia_ui_pointer::EventPhase::kChange, 0);
    ExpectLocationPhaseAndSlot(events[1], 0.0, 0.0, fuchsia_ui_pointer::EventPhase::kRemove, 0);
    ExpectBtnTouch(events[2], 0);
  }

  WaitForMessageFromInputDump(out_socket, relay_api::kWaitForStdinMessage);

  WriteMessageToSocket(in_socket, "quit");
}

}  // namespace
