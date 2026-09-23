// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_GAIN_CONTROL_H_
#define SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_GAIN_CONTROL_H_

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include <gtest/gtest.h>

namespace media::audio::tests {

class FakeGainControl : public fidl::Server<fuchsia_media_audio::GainControl> {
 public:
  FakeGainControl(async_dispatcher_t* dispatcher,
                  fidl::ServerEnd<fuchsia_media_audio::GainControl> server_end) {
    binding_ref_ = fidl::BindServer(
        dispatcher, std::move(server_end), this,
        [this](fidl::Server<fuchsia_media_audio::GainControl>* impl, fidl::UnbindInfo info,
               fidl::ServerEnd<fuchsia_media_audio::GainControl> server_end) {
          unbind_completed_ = true;
        });
  }

  bool UnbindCompleted() const { return unbind_completed_; }

  ~FakeGainControl() override = default;

  // Disallow copy, assign and move.
  FakeGainControl(const FakeGainControl&) = delete;
  FakeGainControl& operator=(const FakeGainControl&) = delete;
  FakeGainControl(FakeGainControl&&) = delete;
  FakeGainControl& operator=(FakeGainControl&&) = delete;

  void Unbind() {
    if (binding_ref_) {
      binding_ref_->Unbind();
    }
  }

  // fuchsia_media_audio::GainControl implementation.
  void SetGain(SetGainRequest& request, SetGainCompleter::Sync& completer) override {
    EXPECT_FALSE(set_gain_artifact_.has_value());
    set_gain_artifact_ = request.gain_db();
  }

  void SetGainWithRamp(SetGainWithRampRequest& request,
                       SetGainWithRampCompleter::Sync& completer) override {
    FX_NOTIMPLEMENTED();
  }

  void SetMute(SetMuteRequest& request, SetMuteCompleter::Sync& completer) override {
    EXPECT_FALSE(set_mute_artifact_.has_value());
    set_mute_artifact_ = request.muted();
  }

  // Checks
  bool WasSetGainCalled(float expected_gain_db) {
    EXPECT_TRUE(set_gain_artifact_.has_value());
    if (!set_gain_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_gain_db, set_gain_artifact_.value());
    bool result = expected_gain_db == set_gain_artifact_.value();
    set_gain_artifact_.reset();
    return result;
  }

  bool WasSetMuteCalled(bool expected_muted) {
    EXPECT_TRUE(set_mute_artifact_.has_value());
    if (!set_mute_artifact_.has_value()) {
      return false;
    }

    EXPECT_EQ(expected_muted, set_mute_artifact_.value());
    bool result = expected_muted == set_mute_artifact_.value();
    set_mute_artifact_.reset();
    return result;
  }

 private:
  std::optional<fidl::ServerBindingRef<fuchsia_media_audio::GainControl>> binding_ref_;
  bool unbind_completed_ = false;

  std::optional<float> set_gain_artifact_;
  std::optional<bool> set_mute_artifact_;
};

}  // namespace media::audio::tests

#endif  // SRC_MEDIA_AUDIO_CONSUMER_TEST_FAKE_GAIN_CONTROL_H_
