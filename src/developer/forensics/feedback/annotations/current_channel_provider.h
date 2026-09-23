// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_CURRENT_CHANNEL_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_CURRENT_CHANNEL_PROVIDER_H_

#include <fidl/fuchsia.update.channel/cpp/fidl.h>

#include "src/developer/forensics/feedback/annotations/fidl_provider.h"
#include "src/developer/forensics/feedback/annotations/types.h"

namespace forensics::feedback {

namespace internal {

inline auto GetCurrent(fidl::Client<fuchsia_update_channel::Provider>& client) {
  return client->GetCurrent();
}

}  // namespace internal

struct CurrentChannelToAnnotations {
  Annotations operator()(const fuchsia_update_channel::ProviderGetCurrentResponse& response);
  Annotations operator()(Error error);
};

// Responsible for collecting annotations for
// fuchsia.update.channel/Provider::GetCurrent.
class CurrentChannelProvider
    : public StaticSingleFidlMethodAnnotationProvider<
          fuchsia_update_channel::Provider, &internal::GetCurrent, CurrentChannelToAnnotations> {
 public:
  using StaticSingleFidlMethodAnnotationProvider::StaticSingleFidlMethodAnnotationProvider;

  virtual ~CurrentChannelProvider() = default;

  static std::set<std::string> GetAnnotationKeys();
  std::set<std::string> GetKeys() const override;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_CURRENT_CHANNEL_PROVIDER_H_
