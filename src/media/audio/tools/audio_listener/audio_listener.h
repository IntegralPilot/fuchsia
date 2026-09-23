// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_TOOLS_AUDIO_LISTENER_AUDIO_LISTENER_H_
#define SRC_MEDIA_AUDIO_TOOLS_AUDIO_LISTENER_AUDIO_LISTENER_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <poll.h>

#include <memory>

#include "src/lib/fsl/tasks/fd_waiter.h"
#include "src/media/audio/tools/audio_listener/escape_decoder.h"

namespace media {

// Future ideas:
// * Watch for device arrivals/departures
// Show DeviceInfo for each device
// * Maybe add watcher for plug/unplug state?
// * Watch for default-device changes (not really used currently)
// * Watch for device gain (not used currently)

class AudioListener;

class UsageGainListenerImpl : public fuchsia::media::UsageGainListener {
 public:
  UsageGainListenerImpl(AudioListener* parent, std::string device_str,
                        fuchsia::media::Usage2 usage);

  fidl::InterfaceHandle<fuchsia::media::UsageGainListener> NewBinding() {
    return binding_.NewBinding();
  }

  AudioListener* parent() { return parent_; }
  fidl::Binding<fuchsia::media::UsageGainListener>& binding() { return binding_; }

  bool muted() const { return muted_; }
  float gain_db() const { return gain_db_; }

 private:
  // |fuchsia::media::UsageGainListener|
  void OnGainMuteChanged(bool muted, float gain_dbfs, OnGainMuteChangedCallback callback) final;

  AudioListener* parent_;
  fidl::Binding<fuchsia::media::UsageGainListener> binding_{this};
  std::string device_str_;
  fuchsia::media::Usage2 usage_;

  bool muted_ = false;
  float gain_db_ = 0.0;
  std::string usage_str_;
};

class UsageWatcherImpl : public fuchsia::media::UsageWatcher2 {
 public:
  UsageWatcherImpl(AudioListener* parent, fuchsia::media::Usage2 usage);

  fidl::InterfaceHandle<fuchsia::media::UsageWatcher2> NewBinding() {
    return binding_.NewBinding();
  }

  AudioListener* parent() { return parent_; }
  fidl::Binding<fuchsia::media::UsageWatcher2>& binding() { return binding_; }

  const fuchsia::media::Usage2& usage() const { return usage_; }
  const fuchsia::media::UsageState& usage_state() const { return usage_state_; }
  std::string usage_state_str() const;
  void set_active() { active_ = true; }
  void clear_active() { active_ = false; }
  bool active() const { return active_; }

 private:
  // |fuchsia::media::UsageWatcher2|
  void OnStateChanged(fuchsia::media::Usage2 usage, fuchsia::media::UsageState usage_state,
                      OnStateChangedCallback callback) override;

  AudioListener* parent_;
  fidl::Binding<fuchsia::media::UsageWatcher2> binding_;
  fuchsia::media::Usage2 usage_;
  fuchsia::media::UsageState usage_state_;
  std::string usage_str_;
  bool active_;
};

class AudioListener {
 public:
  struct Usage {
    fuchsia::media::Usage2 usage;
    std::string short_name;
  };

  AudioListener(int argc, const char** argv, fit::closure quit_callback);
  void Run();

  void RefreshDisplay();

 private:
  enum DisplayMode : uint8_t { UsageActive, UsageState, UsageVolume, UsageGain };
  static void DisplayHeader();

  void WatchRenderActivity();
  void WatchCaptureActivity();
  void OnRenderActivity(const std::vector<fuchsia::media::AudioRenderUsage2>& render_usages);
  void OnCaptureActivity(const std::vector<fuchsia::media::AudioCaptureUsage2>& capture_usages);
  void DisplayUsageActivity();

  void WatchUsageStates();
  void DisplayUsageStates();

  void WatchUsageVolumes();
  void DisplayUsageVolumes();

  void WatchUsageGains();
  void DisplayUsageGains();

  void WaitForKeystroke();
  void HandleKeystroke();

  std::unique_ptr<sys::ComponentContext> component_context_;
  fit::closure quit_callback_;

  fsl::FDWaiter fd_waiter_;
  EscapeDecoder esc_decoder_;

  fuchsia::media::ActivityReporterPtr activity_reporter_;

  fuchsia::media::UsageReporterPtr usage_reporter_;
  // UsageWatchers are stored in alphabetical order of their usage.
  std::unique_ptr<UsageWatcherImpl> render_usage_watchers_[fuchsia::media::RENDER_USAGE2_COUNT];
  std::unique_ptr<UsageWatcherImpl> capture_usage_watchers_[fuchsia::media::CAPTURE_USAGE2_COUNT];

  fuchsia::media::AudioCorePtr audio_core_;
  // VolumeControls, volumes and mutes are stored in alphabetical order of their usage.
  std::array<fuchsia::media::audio::VolumeControlPtr, fuchsia::media::RENDER_USAGE2_COUNT>
      render_usage_volume_ctls_;
  std::array<float, fuchsia::media::RENDER_USAGE2_COUNT> render_usage_volumes_;
  std::array<bool, fuchsia::media::RENDER_USAGE2_COUNT> render_usage_mutes_;

  fuchsia::media::UsageGainReporterPtr usage_gain_reporter_;
  // UsageGainListeners are stored in alphabetical order of their usage.
  std::unique_ptr<UsageGainListenerImpl>
      render_usage_gain_listeners_[fuchsia::media::RENDER_USAGE2_COUNT];
  std::unique_ptr<UsageGainListenerImpl>
      capture_usage_gain_listeners_[fuchsia::media::CAPTURE_USAGE2_COUNT];

  DisplayMode display_mode_ = DisplayMode::UsageActive;
};

}  // namespace media

#endif  // SRC_MEDIA_AUDIO_TOOLS_AUDIO_LISTENER_AUDIO_LISTENER_H_
