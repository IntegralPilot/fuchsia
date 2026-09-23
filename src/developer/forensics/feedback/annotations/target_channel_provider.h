// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_TARGET_CHANNEL_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_TARGET_CHANNEL_PROVIDER_H_

#include <fidl/fuchsia.update.channelcontrol/cpp/fidl.h>

#include "src/developer/forensics/feedback/annotations/fidl_provider.h"
#include "src/developer/forensics/feedback/annotations/types.h"

namespace forensics::feedback {

namespace internal {

inline auto GetTarget(fidl::Client<fuchsia_update_channelcontrol::ChannelControl>& client) {
  return client->GetTarget();
}

}  // namespace internal

struct TargetChannelToAnnotations {
  Annotations operator()(
      const fuchsia_update_channelcontrol::ChannelControlGetTargetResponse& response);
  Annotations operator()(Error error);
};

// Responsible for collecting annotations for
// fuchsia.update.channelcontrol/ChannelControl::GetTarget.
class TargetChannelProvider : public DynamicSingleFidlMethodAnnotationProvider<
                                  fuchsia_update_channelcontrol::ChannelControl,
                                  &internal::GetTarget, TargetChannelToAnnotations> {
 public:
  using DynamicSingleFidlMethodAnnotationProvider::DynamicSingleFidlMethodAnnotationProvider;

  virtual ~TargetChannelProvider() = default;

  static std::set<std::string> GetAnnotationKeys();
  std::set<std::string> GetKeys() const override;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_TARGET_CHANNEL_PROVIDER_H_
