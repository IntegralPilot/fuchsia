// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/board_info_provider.h"

#include <fidl/fuchsia.hwinfo/cpp/natural_types.h>

#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::feedback {

Annotations BoardInfoToAnnotations::operator()(
    const fuchsia_hwinfo::BoardGetInfoResponse& response) {
  const fuchsia_hwinfo::BoardInfo& info = response.info();

  Annotations annotations = operator()(Error::kMissingValue);

  if (info.name().has_value()) {
    annotations.insert_or_assign(kHardwareBoardNameKey, ErrorOrString(*info.name()));
  }

  if (info.revision().has_value()) {
    annotations.insert_or_assign(kHardwareBoardRevisionKey, ErrorOrString(*info.revision()));
  }

  return annotations;
}

Annotations BoardInfoToAnnotations::operator()(const Error error) {
  return Annotations{
      {kHardwareBoardNameKey, ErrorOrString(error)},
      {kHardwareBoardRevisionKey, ErrorOrString(error)},
  };
}

std::set<std::string> BoardInfoProvider::GetAnnotationKeys() {
  return {
      kHardwareBoardNameKey,
      kHardwareBoardRevisionKey,
  };
}

std::set<std::string> BoardInfoProvider::GetKeys() const {
  return BoardInfoProvider::GetAnnotationKeys();
}

}  // namespace forensics::feedback
