// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/decode.h"

#include <fuchsia/feedback/cpp/fidl.h>

namespace forensics::feedback {

Annotations FromFidl(const std::vector<fuchsia::feedback::Annotation>& fidl) {
  Annotations annotations;
  for (const fuchsia::feedback::Annotation& annotation : fidl) {
    annotations.insert({annotation.key, ErrorOrString(annotation.value)});
  }

  return annotations;
}

}  // namespace forensics::feedback
