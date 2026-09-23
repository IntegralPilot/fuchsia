// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/activity_dispatcher.h"

#include <fuchsia/media/cpp/fidl.h>

#include <optional>

namespace media::audio {

namespace {

using fuchsia::media::AudioCaptureUsage;
using fuchsia::media::AudioCaptureUsage2;
using fuchsia::media::AudioRenderUsage;
using fuchsia::media::AudioRenderUsage2;

std::vector<AudioRenderUsage> ActivityToRenderUsageVector(
    const ActivityDispatcherImpl::RenderActivity& activity) {
  std::vector<AudioRenderUsage> usage_vector;
  usage_vector.reserve(activity.count());

  for (uint8_t i = 0u; i < fuchsia::media::RENDER_USAGE_COUNT; i++) {
    if (activity[i]) {
      usage_vector.push_back(AudioRenderUsage(i));
    }
  }
  return usage_vector;
}

std::vector<AudioRenderUsage2> ActivityToRenderUsage2Vector(
    const ActivityDispatcherImpl::RenderActivity& activity) {
  std::vector<AudioRenderUsage2> usage_vector;
  usage_vector.reserve(activity.count());

  for (uint8_t i = 0u; i < fuchsia::media::RENDER_USAGE2_COUNT; i++) {
    if (activity[i]) {
      usage_vector.emplace_back(i);
    }
  }
  return usage_vector;
}

std::vector<AudioCaptureUsage> ActivityToCaptureUsageVector(
    const ActivityDispatcherImpl::CaptureActivity& activity) {
  std::vector<AudioCaptureUsage> usage_vector;
  usage_vector.reserve(activity.count());

  for (uint8_t i = 0u; i < fuchsia::media::CAPTURE_USAGE_COUNT; i++) {
    if (activity[i]) {
      usage_vector.push_back(AudioCaptureUsage(i));
    }
  }
  return usage_vector;
}

std::vector<AudioCaptureUsage2> ActivityToCaptureUsage2Vector(
    const ActivityDispatcherImpl::CaptureActivity& activity) {
  std::vector<AudioCaptureUsage2> usage_vector;
  usage_vector.reserve(activity.count());

  for (uint8_t i = 0u; i < fuchsia::media::CAPTURE_USAGE2_COUNT; i++) {
    if (activity[i]) {
      usage_vector.emplace_back(i);
    }
  }
  return usage_vector;
}

}  // namespace

class ActivityDispatcherImpl::ActivityReporterImpl : public fuchsia::media::ActivityReporter {
 public:
  // The activity must outlive the ActivityReporterImpl.
  explicit ActivityReporterImpl(const RenderActivity& last_known_render_activity,
                                const CaptureActivity& last_known_capture_activity,
                                fit::callback<void(ActivityReporterImpl*)> on_client_error);
  ~ActivityReporterImpl() override;

  // Signal that the activity changed.
  void OnRenderActivityChanged();
  void OnCaptureActivityChanged();

  // Handle unresponsive client.
  void OnClientError();

  // |fuchsia::media::ActivityReporter|
  void WatchRenderActivity(WatchRenderActivityCallback callback) override;
  void WatchCaptureActivity(WatchCaptureActivityCallback callback) override;
  void WatchRenderActivity2(WatchRenderActivity2Callback callback) override;
  void WatchCaptureActivity2(WatchCaptureActivity2Callback callback) override;

 protected:
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {
    FX_LOGS(ERROR) << "ActivityReporterImpl: ActivityReporter::handle_unknown_method(ordinal "
                   << ordinal << ", method_has_response " << method_has_response << ")";
  }

 private:
  // The legacy WatchRenderActivity method is only aware of the first
  // five render usages. For those two methods only, we mask off
  // any other usages from the vectors we return.
  static constexpr uint8_t kLegacyRenderActivityBitmask = 0b00011111;

  void MaybeSendRenderActivity();
  void MaybeSendCaptureActivity();
  void MaybeSendRenderActivity2();
  void MaybeSendCaptureActivity2();

  const RenderActivity& last_known_render_activity_;
  const CaptureActivity& last_known_capture_activity_;

  // Last activity sent to the client on that interface.
  // Absent if no state was sent on that interface to the client yet.
  std::optional<RenderActivity> last_sent_render_activity_;
  std::optional<CaptureActivity> last_sent_capture_activity_;
  std::optional<RenderActivity> last_sent_render_activity_2_;
  std::optional<CaptureActivity> last_sent_capture_activity_2_;

  // If present, callback to call next time a state is available.
  WatchRenderActivityCallback waiting_render_activity_callback_;
  WatchCaptureActivityCallback waiting_capture_activity_callback_;
  WatchRenderActivity2Callback waiting_render_activity_2_callback_;
  WatchCaptureActivity2Callback waiting_capture_activity_2_callback_;

  // Called when the client has more than one hanging get in flight for a single interface.
  fit::callback<void(ActivityReporterImpl*)> on_client_error_;
};

ActivityDispatcherImpl::ActivityDispatcherImpl() = default;
ActivityDispatcherImpl::~ActivityDispatcherImpl() = default;

ActivityDispatcherImpl::ActivityReporterImpl::ActivityReporterImpl(
    const RenderActivity& last_known_render_activity,
    const CaptureActivity& last_known_capture_activity,
    fit::callback<void(ActivityReporterImpl*)> on_client_error)
    : last_known_render_activity_(last_known_render_activity),
      last_known_capture_activity_(last_known_capture_activity),
      on_client_error_(std::move(on_client_error)) {}

ActivityDispatcherImpl::ActivityReporterImpl::~ActivityReporterImpl() = default;

void ActivityDispatcherImpl::Bind(
    fidl::InterfaceRequest<fuchsia::media::ActivityReporter> request) {
  constexpr auto kEpitaphValue = ZX_ERR_PEER_CLOSED;
  bindings_.AddBinding(
      std::make_unique<ActivityReporterImpl>(
          last_known_render_activity_, last_known_capture_activity_,
          [this](ActivityReporterImpl* impl) { bindings_.CloseBinding(impl, kEpitaphValue); }),
      std::move(request));
}

fidl::InterfaceRequestHandler<fuchsia::media::ActivityReporter>
ActivityDispatcherImpl::GetFidlRequestHandler() {
  return fit::bind_member<&ActivityDispatcherImpl::Bind>(this);
}

void ActivityDispatcherImpl::ActivityReporterImpl::OnClientError() { on_client_error_(this); }

// All methods below are mirrored for Render and Capture.

// The set of active Render usages has changed. Check whether we should immediately respond.
void ActivityDispatcherImpl::ActivityReporterImpl::OnRenderActivityChanged() {
  MaybeSendRenderActivity();
  MaybeSendRenderActivity2();
}

void ActivityDispatcherImpl::ActivityReporterImpl::OnCaptureActivityChanged() {
  MaybeSendCaptureActivity();
  MaybeSendCaptureActivity2();
}

// If there is more than one hanging get in flight, disconnect the client.
// Otherwise, save the callback and check whether we should immediately respond.
void ActivityDispatcherImpl::ActivityReporterImpl::WatchRenderActivity(
    WatchRenderActivityCallback callback) {
  if (waiting_render_activity_callback_) {
    OnClientError();
    return;
  }

  waiting_render_activity_callback_ = std::move(callback);
  MaybeSendRenderActivity();
}

void ActivityDispatcherImpl::ActivityReporterImpl::WatchCaptureActivity(
    WatchCaptureActivityCallback callback) {
  if (waiting_capture_activity_callback_) {
    OnClientError();
    return;
  }

  waiting_capture_activity_callback_ = std::move(callback);
  MaybeSendCaptureActivity();
}

void ActivityDispatcherImpl::ActivityReporterImpl::WatchRenderActivity2(
    WatchRenderActivity2Callback callback) {
  if (waiting_render_activity_2_callback_) {
    OnClientError();
    return;
  }

  waiting_render_activity_2_callback_ = std::move(callback);
  MaybeSendRenderActivity2();
}

void ActivityDispatcherImpl::ActivityReporterImpl::WatchCaptureActivity2(
    WatchCaptureActivity2Callback callback) {
  if (waiting_capture_activity_2_callback_) {
    OnClientError();
    return;
  }

  waiting_capture_activity_2_callback_ = std::move(callback);
  MaybeSendCaptureActivity2();
}

// If no request in flight, just return. If no change since last request, just return.
// If there IS a change, or if this is the first request, then we will respond: convert bitmask of
// activities into vector of usages and invoke the callback.
//
// Note that when checking for change, we limit the activity set to only legacy ones. For method
// WatchRenderActivity, only legacy usages trigger a change and only legacy usages are returned.
void ActivityDispatcherImpl::ActivityReporterImpl::MaybeSendRenderActivity() {
  if (!waiting_render_activity_callback_) {
    return;
  }

  // We only do this for Render (not Capture), as AudioRenderUsage2 contains additional value(s)
  // not found in AudioRenderUsage; the legacy WatchRenderActivity method should not return them.
  auto last_known_legacy_render_activity = last_known_render_activity_;
  last_known_legacy_render_activity &= kLegacyRenderActivityBitmask;

  if (last_sent_render_activity_.has_value() &&
      last_sent_render_activity_.value() == last_known_legacy_render_activity) {
    return;
  }

  auto callback = std::move(waiting_render_activity_callback_);
  waiting_render_activity_callback_ = nullptr;
  last_sent_render_activity_ = last_known_legacy_render_activity;
  callback(ActivityToRenderUsageVector(last_known_legacy_render_activity));
}

// If no request in flight, just return. If no change since last request, just return.
// If there IS a change, or if this is the first request, then we will respond: convert bitmask of
// activities into vector of usages and invoke the callback.
void ActivityDispatcherImpl::ActivityReporterImpl::MaybeSendCaptureActivity() {
  if (!waiting_capture_activity_callback_) {
    return;
  }

  if (last_sent_capture_activity_.has_value() &&
      last_sent_capture_activity_.value() == last_known_capture_activity_) {
    return;
  }

  auto callback = std::move(waiting_capture_activity_callback_);
  waiting_capture_activity_callback_ = nullptr;
  last_sent_capture_activity_ = last_known_capture_activity_;
  callback(ActivityToCaptureUsageVector(last_known_capture_activity_));
}

// If no request in flight, just return. If no change since last request, just return.
// If there IS a change, or if this is the first request, then we will respond: convert bitmask of
// activities into vector of usages and invoke the callback.
//
// Identical to MaybeSendRenderActivity, except (1) we don't mask off the non-legacy usages,
// and (2) when invoking the callback, we wrap the response vector in a fidl::Result.
// For method WatchRenderActivity2, all usages trigger a change and all usages are returned.
void ActivityDispatcherImpl::ActivityReporterImpl::MaybeSendRenderActivity2() {
  if (!waiting_render_activity_2_callback_) {
    return;
  }

  if (last_sent_render_activity_2_.has_value() &&
      last_sent_render_activity_2_.value() == last_known_render_activity_) {
    return;
  }

  auto callback = std::move(waiting_render_activity_2_callback_);
  waiting_render_activity_2_callback_ = nullptr;
  last_sent_render_activity_2_ = last_known_render_activity_;
  callback(fpromise::ok(ActivityToRenderUsage2Vector(last_known_render_activity_)));
}

// If no request in flight, just return. If no change since last request, just return.
// If there IS a change, or if this is the first request, then we will respond: convert bitmask of
// activities into vector of usages and invoke the callback.
//
// Identical to MaybeSendCaptureActivity, except when invoking the callback, we wrap the response
// vector in a fidl::Result.
void ActivityDispatcherImpl::ActivityReporterImpl::MaybeSendCaptureActivity2() {
  if (!waiting_capture_activity_2_callback_) {
    return;
  }

  if (last_sent_capture_activity_2_.has_value() &&
      last_sent_capture_activity_2_.value() == last_known_capture_activity_) {
    return;
  }

  auto callback = std::move(waiting_capture_activity_2_callback_);
  waiting_capture_activity_2_callback_ = nullptr;
  last_sent_capture_activity_2_ = last_known_capture_activity_;
  callback(fpromise::ok(ActivityToCaptureUsage2Vector(last_known_capture_activity_)));
}

// The set of active Render usages has changed. Notify all connected ActivityReporter clients.
void ActivityDispatcherImpl::OnRenderActivityChanged(RenderActivity activity) {
  last_known_render_activity_ = activity;
  for (const auto& listener : bindings_.bindings()) {
    listener->impl()->OnRenderActivityChanged();
  }
}

void ActivityDispatcherImpl::OnCaptureActivityChanged(CaptureActivity activity) {
  last_known_capture_activity_ = activity;
  for (const auto& listener : bindings_.bindings()) {
    listener->impl()->OnCaptureActivityChanged();
  }
}

}  // namespace media::audio
