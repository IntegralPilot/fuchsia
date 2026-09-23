// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/tests/basic_test.h"

#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl.h>
#include <lib/fdio/fdio.h>
#include <lib/fidl/cpp/enum.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/compiler.h>

#include <cmath>
#include <optional>
#include <string>

#include <gtest/gtest.h>

#include "src/media/audio/drivers/tests/test_base.h"

namespace media::audio::drivers::test {

namespace {

inline constexpr bool kLogGainValues = false;

std::string to_string(const fuchsia::hardware::audio::GainState& gain_state) {
  std::string gain_db_str;
  std::string mute_str;
  std::string agc_str;
  if (gain_state.has_gain_db()) {
    gain_db_str = std::string("   ") + std::to_string(gain_state.gain_db()).substr(0, 6);
    gain_db_str = gain_db_str.substr(gain_db_str.length() - 6, 6);
  } else {
    gain_db_str = "[NONE]";
  }
  if (gain_state.has_muted()) {
    mute_str = gain_state.muted() ? " true " : "false ";
  } else {
    mute_str = "[NONE]";
  }
  if (gain_state.has_agc_enabled()) {
    agc_str = gain_state.agc_enabled() ? " enabled" : "disabled";
  } else {
    agc_str = "  [NONE]";
  }

  return gain_db_str.append(" dB, muted is ").append(mute_str).append(", AGC is ").append(agc_str);
}

void LogGainState(const std::string& prologue,
                  const fuchsia::hardware::audio::GainState& gain_state) {
  if constexpr (kLogGainValues) {
    FX_LOGS(INFO) << prologue << to_string(gain_state);
  }
}

}  // namespace

void BasicTest::TearDown() {
  // Restore initial_gain_state_, if we changed the gain in this test case.
  if (stream_config().is_bound() && initial_gain_state_.has_value() &&
      expected_gain_state_.has_value()) {
    LogGainState("Restore previous gain: ", *initial_gain_state_);
    stream_config()->SetGain(std::move(*initial_gain_state_));
    initial_gain_state_.reset();

    stream_config()->WatchGainState(
        AddCallback("WatchGainState", [](fuchsia::hardware::audio::GainState gain_state) {
          LogGainState("TearDown- gain became: ", gain_state);
        }));
    ExpectCallbacks();
  }

  TestBase::TearDown();
}

// Basic (non-privileged) requests
//
// Request that the driver return its gain capabilities and current state, expecting a response.
// TODO(b/315051281): If possible, combine this with the corresponding check of the signalprocessing
// gain element, once that test exists.
void BasicTest::WatchGainStateAndExpectUpdate() {
  ASSERT_TRUE(properties().has_value());
  ASSERT_TRUE(device_entry().isStreamConfig());

  // We reconnect the stream every time we run a test. Per driver interface definition, the driver
  // must reply to the first watch request, so we get gain state by issuing a watch FIDL call.
  stream_config()->WatchGainState(
      AddCallback("WatchGainState", [this](fuchsia::hardware::audio::GainState gain_state) {
        LogGainState((initial_gain_state_.has_value() ? "Received gain update:  "
                                                      : "Storing initial gain:  "),
                     gain_state);

        ASSERT_TRUE(gain_state.has_gain_db());
        EXPECT_GE(gain_state.gain_db(), min_gain_db());
        EXPECT_LE(gain_state.gain_db(), max_gain_db());

        if (!initial_gain_state_.has_value()) {
          if (expected_gain_state_.has_value()) {
            FX_LOGS(ERROR)
                << "*** Unexpected: initial_gain_state_ not set, but expected_gain_state_ is";
          }
          initial_gain_state_ = std::move(gain_state);
        }

        // If we're muted, then we must be capable of muting.
        EXPECT_TRUE(!gain_state.has_muted() || !gain_state.muted() || *properties()->can_mute);
        // If AGC is enabled, then we must be capable of AGC.
        EXPECT_TRUE(!gain_state.has_agc_enabled() || !gain_state.agc_enabled() ||
                    *properties()->can_agc);
        if (expected_gain_state_.has_value()) {
          EXPECT_EQ(gain_state.gain_db(), expected_gain_state_->gain_db());
          EXPECT_EQ(gain_state.has_muted() && gain_state.muted(),
                    expected_gain_state_->has_muted() && expected_gain_state_->muted());
          EXPECT_EQ(gain_state.has_agc_enabled() && gain_state.agc_enabled(),
                    expected_gain_state_->has_agc_enabled() && expected_gain_state_->agc_enabled());
        }
      }));
  ExpectCallbacks();
}

// Request that the driver return its current gain state, expecting no response (no change).
// TODO(b/315051281): If possible, combine this with the corresponding check of the signalprocessing
// gain element, once that test exists.
void BasicTest::WatchGainStateAndExpectNoUpdate() {
  ASSERT_TRUE(properties().has_value());
  ASSERT_TRUE(initial_gain_state_.has_value());
  stream_config()->WatchGainState([](const fuchsia::hardware::audio::GainState& gain_state) {
    FAIL() << "Received unexpected gain:      " << to_string(gain_state);
  });
}

// Determine an appropriate gain state to request, then call other method to request that driver set
// gain. This method assumes that the driver already successfully responded to a GetInitialGainState
// request. If this device's gain is fixed and cannot be changed, then SKIP the test.
// TODO(b/315051281): If possible, combine this with the corresponding check of the signalprocessing
// gain element, once that test exists.
void BasicTest::SetGainStateChange() {
  ASSERT_TRUE(device_entry().isStreamConfig()) << __func__ << ": device_entry is not StreamConfig";
  ASSERT_TRUE(properties().has_value());
  ASSERT_TRUE(properties()->max_gain_db.has_value());
  ASSERT_TRUE(properties()->min_gain_db.has_value());

  if (properties()->max_gain_db == properties()->min_gain_db &&
      !properties()->can_mute.value_or(false) && !properties()->can_agc.value_or(false)) {
    GTEST_SKIP() << "*** Audio " << driver_type() << " has fixed gain ("
                 << *properties()->max_gain_db
                 << " dB) and cannot MUTE or AGC. Skipping SetGain test. ***";
  }

  // Ensure we've retrieved initial gain settings, so we can restore them after this test case.
  ASSERT_TRUE(initial_gain_state_.has_value());

  fuchsia::hardware::audio::GainState gain_state_to_set;
  ASSERT_EQ(initial_gain_state_->Clone(&gain_state_to_set), ZX_OK);
  // Base our new gain settings on the old ones: avoid existing values so this Set is a change.
  // If we got this far, we know we can change something (even if it isn't gain_db).
  // Change to a different gain_db.
  *gain_state_to_set.mutable_gain_db() =
      (initial_gain_state_->gain_db() == min_gain_db() ? max_gain_db() : min_gain_db());
  // Toggle muted if we can change it (explicitly set it to false, if we can't).
  *gain_state_to_set.mutable_muted() =
      *properties()->can_mute && !(gain_state_to_set.has_muted() && gain_state_to_set.muted());
  // Toggle AGC if we can change it (explicitly set it to false, if we can't).
  *gain_state_to_set.mutable_agc_enabled() =
      *properties()->can_agc &&
      !(gain_state_to_set.has_agc_enabled() && gain_state_to_set.agc_enabled());
  // Save this new GainState for comparison to the expected gain-change notification.
  expected_gain_state_ = fuchsia::hardware::audio::GainState{};
  ASSERT_EQ(gain_state_to_set.Clone(&expected_gain_state_.value()), ZX_OK);

  RequestSetGain(std::move(gain_state_to_set));
}

// Call SetGain with the current gain state.
// Because we expect this to be ignored by the audio driver, we do not set expected_gain_state_.
void BasicTest::SetGainStateNoChange() {
  ASSERT_TRUE(initial_gain_state_.has_value());
  ASSERT_FALSE(expected_gain_state_.has_value());

  fuchsia::hardware::audio::GainState gain_state_to_set;
  ASSERT_EQ(initial_gain_state_->Clone(&gain_state_to_set), ZX_OK);
  RequestSetGain(std::move(gain_state_to_set));
}

// Call SetGain without setting `gain_db`, `muted` or `agc_enabled`.
// Because we expect this to be ignored by the audio driver, we do not set expected_gain_state_.
void BasicTest::SetGainStateNoValues() {
  ASSERT_TRUE(initial_gain_state_.has_value());
  ASSERT_FALSE(expected_gain_state_.has_value());

  fuchsia::hardware::audio::GainState gain_state_to_set;
  gain_state_to_set.clear_gain_db();
  gain_state_to_set.clear_muted();
  gain_state_to_set.clear_agc_enabled();
  RequestSetGain(std::move(gain_state_to_set));
}

// Because this sets `gain_db` values that should be ignored by the audio driver (and we do NOT set
// `muted` or `agc_enabled`), we do not set expected_gain_state_.
void BasicTest::SetImpossibleGainDb(float gain_db) {
  ASSERT_TRUE(initial_gain_state_.has_value());

  // Base our MUTE/AGC settings on the old ones. Other than gain_db, this represents no change.
  fuchsia::hardware::audio::GainState gain_state_to_set;
  ASSERT_EQ(initial_gain_state_->Clone(&gain_state_to_set), ZX_OK);
  *gain_state_to_set.mutable_gain_db() = gain_db;

  RequestSetGain(std::move(gain_state_to_set));
}

// Set audio driver MUTE to an invalid setting: enable it, if the driver does not support it.
// Because we expect this to be ignored by the audio driver, we do not set expected_gain_state_.
void BasicTest::SetImpossibleMute() {
  ASSERT_TRUE(properties().has_value());
  ASSERT_TRUE(initial_gain_state_.has_value());

  if (properties()->can_mute.value_or(false)) {
    GTEST_SKIP() << "*** Audio " << driver_type() << " can MUTE. Skipping SetBadMute test. ***";
    __UNREACHABLE;
  }

  // Base our new gain settings on the old ones. Other than Mute, this represents no gain change.
  fuchsia::hardware::audio::GainState gain_state_to_set;
  ASSERT_EQ(initial_gain_state_->Clone(&gain_state_to_set), ZX_OK);
  *gain_state_to_set.mutable_muted() = true;

  RequestSetGain(std::move(gain_state_to_set));
}

// Set audio driver AGC to an invalid setting: enable it, if the driver does not support it.
// Because we expect this to be ignored by the audio driver, we do not set expected_gain_state_.
void BasicTest::SetImpossibleAgc() {
  ASSERT_TRUE(properties().has_value());
  ASSERT_TRUE(initial_gain_state_.has_value());

  if (properties()->can_agc.value_or(false)) {
    GTEST_SKIP() << "*** Audio " << driver_type()
                 << " can enable/disable AGC. Skipping SetBadAgc test. ***";
    __UNREACHABLE;
  }

  // Base our new gain settings on the old ones. Other than AGC, this represents no gain change.
  fuchsia::hardware::audio::GainState gain_state_to_set;
  ASSERT_EQ(initial_gain_state_->Clone(&gain_state_to_set), ZX_OK);
  *gain_state_to_set.mutable_agc_enabled() = true;

  RequestSetGain(std::move(gain_state_to_set));
}

void BasicTest::RequestSetGain(fuchsia::hardware::audio::GainState gain_state) {
  if (!device_entry().isStreamConfig()) {
    FAIL() << "device_entry is not StreamConfig";
    return;
  }

  LogGainState("SetGain about to set:  ", gain_state);
  stream_config()->SetGain(std::move(gain_state));
}

// TODO(b/315051014): If possible, combine this with the corresponding plug check of the
// signalprocessing endpoint element, once that test exists.
void BasicTest::ValidatePlugState(const fuchsia::hardware::audio::PlugState& plug_state) {
  ASSERT_TRUE(plug_state.has_plugged());
  if (!plug_state.plugged()) {
    ASSERT_TRUE(properties().has_value());
    ASSERT_TRUE(properties()->plug_detect_capabilities.has_value());
    EXPECT_NE(*properties()->plug_detect_capabilities,
              fuchsia::hardware::audio::PlugDetectCapabilities::HARDWIRED)
        << "Device reported plug capabilities as HARDWIRED, but now reports as unplugged";
  }

  EXPECT_TRUE(plug_state.has_plug_state_time());
  EXPECT_GE(plug_state.plug_state_time(), 0u);
  EXPECT_LT(plug_state.plug_state_time(), zx::clock::get_monotonic().get());
}

// Request that the driver return its current plug state, expecting a valid response.
// TODO(b/315051014): If possible, combine this with the corresponding plug check of the
// signalprocessing endpoint element, once that test exists.
void BasicTest::WatchPlugStateAndExpectUpdate() {
  ASSERT_TRUE(properties().has_value());

  // Since we reconnect to the audio stream every time we run this test and we are guaranteed by
  // the audio driver interface definition that the driver will reply to the first watch request,
  // we can get the plug state by issuing a watch FIDL call.
  fuchsia::hardware::audio::PlugState initial_plug_state;
  if (device_entry().isCodec()) {
    codec()->WatchPlugState(AddCallback(
        "Codec::WatchPlugState", [&initial_plug_state](fuchsia::hardware::audio::PlugState state) {
          initial_plug_state = std::move(state);
        }));
  } else if (device_entry().isStreamConfig()) {
    stream_config()->WatchPlugState(
        AddCallback("StreamConfig::WatchPlugState",
                    [&initial_plug_state](fuchsia::hardware::audio::PlugState state) {
                      initial_plug_state = std::move(state);
                    }));
  } else {
    FAIL() << "Wrong device type for " << __func__;
  }
  ExpectCallbacks();
  if (!HasFailure()) {
    ValidatePlugState(initial_plug_state);
  }
}

// Request that the driver return its current plug state, expecting no response (no change).
// TODO(b/315051014): If possible, combine this with the corresponding plug check of the
// signalprocessing endpoint element, once that test exists.
void BasicTest::WatchPlugStateAndExpectNoUpdate() {
  if (device_entry().isCodec()) {
    codec()->WatchPlugState([](fuchsia::hardware::audio::PlugState state) {
      FAIL() << "Codec::WatchPlugState: unexpected plug update received";
    });
  } else if (device_entry().isStreamConfig()) {
    stream_config()->WatchPlugState([](fuchsia::hardware::audio::PlugState state) {
      FAIL() << "StreamConfig::WatchPlugState: unexpected plug update received";
    });
  } else {
    FAIL() << "Wrong device type for " << __func__;
  }
}

#define DEFINE_BASIC_TEST_CLASS(CLASS_NAME, CODE)                               \
  class CLASS_NAME : public BasicTest {                                         \
   public:                                                                      \
    explicit CLASS_NAME(const DeviceEntry& dev_entry) : BasicTest(dev_entry) {} \
    void TestBody() override { CODE }                                           \
  }

// Test cases that target each of the various Stream channel commands

// Verify the driver responds to the GetHealthState query.
DEFINE_BASIC_TEST_CLASS(Health, { RequestHealthAndExpectHealthy(); });

// Verify a valid unique_id, manufacturer, product and gain capabilities is successfully received.
DEFINE_BASIC_TEST_CLASS(GetProperties, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ValidateProperties();
  WaitForError();
});

// Verify the initial WatchGainState responses are successfully received.
DEFINE_BASIC_TEST_CLASS(GetInitialGainState, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());

  WatchGainStateAndExpectUpdate();
  WaitForError();
});

// Verify that no response is received, for a subsequent WatchGainState request.
DEFINE_BASIC_TEST_CLASS(WatchGainSecondTimeNoResponse, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  WatchGainStateAndExpectNoUpdate();
  WaitForError();
});

// Verify valid set gain responses are successfully received.
DEFINE_BASIC_TEST_CLASS(SetGainChangedCausesNotification, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  ASSERT_NO_FAILURE_OR_SKIP(SetGainStateChange());
  WatchGainStateAndExpectUpdate();
  WaitForError();
});

// Verify set gain of the current value does not lead to a gain-change notification.
DEFINE_BASIC_TEST_CLASS(SetGainUnchangedDoesNotCauseNotification, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  ASSERT_NO_FAILURE_OR_SKIP(SetGainStateNoChange());
  WatchGainStateAndExpectNoUpdate();
  WaitForError();
});

// Verify that omitting `gain_db`, `muted` or `agc_enabled` equates to no-change in those fields.
DEFINE_BASIC_TEST_CLASS(SetGainNoValuesMeansNoChange, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  ASSERT_NO_FAILURE_OR_SKIP(SetGainStateNoValues());
  WatchGainStateAndExpectNoUpdate();
  WaitForError();
});

// Verify invalid set gain responses are simply ignored (no disconnect or failed FIDL call).
// Importantly, NO gain-change notification should be emitted.
DEFINE_BASIC_TEST_CLASS(SetGainInvalidGainValuesAreIgnored, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  // For the remaining SetGain calls, we will fail if we EVER receive a gain-change notification.
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectNoUpdate());

  {
    SCOPED_TRACE(testing::Message() << "Testing SetGain for gain_db -Infinity");
    ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleGainDb(-INFINITY));
    ASSERT_NO_FAILURE_OR_SKIP(RequestHealthAndExpectHealthy());
  }

  {
    SCOPED_TRACE(testing::Message() << "Testing SetGain for gain_db +Infinity");
    ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleGainDb(INFINITY));
    ASSERT_NO_FAILURE_OR_SKIP(RequestHealthAndExpectHealthy());
  }

  {
    SCOPED_TRACE(testing::Message() << "Testing SetGain for gain_db Nan");
    ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleGainDb(NAN));
    ASSERT_NO_FAILURE_OR_SKIP(RequestHealthAndExpectHealthy());
  }

  WaitForError();
});

// Verify invalid set gain responses are simply ignored (no disconnect or failed FIDL call).
// Importantly, NO gain-change notification should be emitted.
DEFINE_BASIC_TEST_CLASS(SetGainOutOfRangeGainValuesAreIgnored, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  // For the remaining SetGain calls, we will fail if we EVER receive a gain-change notification.
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectNoUpdate());

  {
    SCOPED_TRACE(testing::Message() << "Testing SetGain for gain_db too low");
    ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleGainDb(min_gain_db() - 1.0f));
    ASSERT_NO_FAILURE_OR_SKIP(RequestHealthAndExpectHealthy());
  }

  {
    SCOPED_TRACE(testing::Message() << "Testing SetGain for gain_db too high");
    ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleGainDb(max_gain_db() + 1.0f));
    ASSERT_NO_FAILURE_OR_SKIP(RequestHealthAndExpectHealthy());
  }

  WaitForError();
});

// Verify invalid set MUTE is simply ignored (no disconnect or failed FIDL call). This is testable
// only if the device cannot MUTE. Importantly, NO gain-change notification should be emitted.
DEFINE_BASIC_TEST_CLASS(SetGainInvalidMuteIsIgnored, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectNoUpdate());
  ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleMute());
  RequestHealthAndExpectHealthy();
  WaitForError();
});

// Verify invalid set AGC is simply ignored (no disconnect or failed FIDL call). This is testable
// only if the device has no AGC. Importantly, NO gain-change notification should be emitted.
DEFINE_BASIC_TEST_CLASS(SetGainInvalidAgcIsIgnored, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectUpdate());

  ASSERT_NO_FAILURE_OR_SKIP(WatchGainStateAndExpectNoUpdate());
  ASSERT_NO_FAILURE_OR_SKIP(SetImpossibleAgc());
  RequestHealthAndExpectHealthy();
  WaitForError();
});

// Verify that format-retrieval responses are successfully received and are complete and valid.
DEFINE_BASIC_TEST_CLASS(RingBufferFormats, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  WaitForError();
});

// Verify that format-retrieval responses are successfully received and are complete and valid.
DEFINE_BASIC_TEST_CLASS(DaiFormats, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveDaiFormats());
  WaitForError();
});

// Verify that a valid initial plug detect response is successfully received.
DEFINE_BASIC_TEST_CLASS(GetInitialPlugState, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());

  WatchPlugStateAndExpectUpdate();
  WaitForError();

  // Someday: determine how to trigger the driver's internal hardware-detect mechanism, so it
  // emits unsolicited PLUG/UNPLUG events -- otherwise driver plug detect updates are not fully
  // testable.
});

// Verify that no response is received, for a subsequent WatchPlugState request.
DEFINE_BASIC_TEST_CLASS(WatchPlugSecondTimeNoResponse, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchPlugStateAndExpectUpdate());

  WatchPlugStateAndExpectNoUpdate();
  WaitForError();
});

// Register separate test case instances for each enumerated device
//
// See googletest/docs/advanced.md for details
#define REGISTER_BASIC_TEST(CLASS_NAME, DEVICE)                                                \
  {                                                                                            \
    testing::RegisterTest("BasicTest", TestNameForEntry(#CLASS_NAME, DEVICE).c_str(), nullptr, \
                          DevNameForEntry(DEVICE).c_str(), __FILE__, __LINE__,                 \
                          [&]() -> BasicTest* { return new CLASS_NAME(DEVICE); });             \
  }

void RegisterBasicTestsForDevice(const DeviceEntry& device_entry) {
  if (device_entry.isCodec()) {
    REGISTER_BASIC_TEST(Health, device_entry);
    REGISTER_BASIC_TEST(GetProperties, device_entry);
    REGISTER_BASIC_TEST(DaiFormats, device_entry);
    REGISTER_BASIC_TEST(GetInitialPlugState, device_entry);
    REGISTER_BASIC_TEST(WatchPlugSecondTimeNoResponse, device_entry);
  } else if (device_entry.isComposite()) {
    // No test cases here; by definition Composite drivers can have
    // only one concurrently-connected client.
  } else if (device_entry.isDai()) {
    REGISTER_BASIC_TEST(Health, device_entry);
    REGISTER_BASIC_TEST(GetProperties, device_entry);
    REGISTER_BASIC_TEST(RingBufferFormats, device_entry);
    REGISTER_BASIC_TEST(DaiFormats, device_entry);
  } else if (device_entry.isStreamConfig()) {
    REGISTER_BASIC_TEST(Health, device_entry);
    REGISTER_BASIC_TEST(GetProperties, device_entry);
    REGISTER_BASIC_TEST(RingBufferFormats, device_entry);
    REGISTER_BASIC_TEST(GetInitialPlugState, device_entry);
    REGISTER_BASIC_TEST(WatchPlugSecondTimeNoResponse, device_entry);

    REGISTER_BASIC_TEST(GetInitialGainState, device_entry);
    REGISTER_BASIC_TEST(WatchGainSecondTimeNoResponse, device_entry);
    REGISTER_BASIC_TEST(SetGainChangedCausesNotification, device_entry);
    REGISTER_BASIC_TEST(SetGainUnchangedDoesNotCauseNotification, device_entry);
    REGISTER_BASIC_TEST(SetGainNoValuesMeansNoChange, device_entry);
    REGISTER_BASIC_TEST(SetGainOutOfRangeGainValuesAreIgnored, device_entry);
    REGISTER_BASIC_TEST(SetGainInvalidGainValuesAreIgnored, device_entry);
    REGISTER_BASIC_TEST(SetGainInvalidMuteIsIgnored, device_entry);
    REGISTER_BASIC_TEST(SetGainInvalidAgcIsIgnored, device_entry);
  } else {
    FAIL() << "Unknown device type for entry '" << device_entry.filename << "'";
  }
}

// TODO(b/302704556): Add tests for Watch-while-still-pending (specifically WatchGainState,
//   WatchPlugState, WatchClockRecoveryPositionInfo and WatchDelayInfo).

}  // namespace media::audio::drivers::test
