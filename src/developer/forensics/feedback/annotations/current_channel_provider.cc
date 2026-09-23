// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/current_channel_provider.h"

#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::feedback {

Annotations CurrentChannelToAnnotations::operator()(
    const fuchsia_update_channel::ProviderGetCurrentResponse& response) {
  return Annotations{
      {kSystemUpdateChannelCurrentKey, ErrorOrString(response.channel())},
  };
}

Annotations CurrentChannelToAnnotations::operator()(const Error error) {
  return Annotations{
      {kSystemUpdateChannelCurrentKey, ErrorOrString(error)},
  };
}

std::set<std::string> CurrentChannelProvider::GetAnnotationKeys() {
  return {
      kSystemUpdateChannelCurrentKey,
  };
}

std::set<std::string> CurrentChannelProvider::GetKeys() const {
  return CurrentChannelProvider::GetAnnotationKeys();
}

}  // namespace forensics::feedback
