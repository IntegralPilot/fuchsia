// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <android-base/properties.h>

namespace android {
namespace base {

std::string GetProperty(const std::string& key, const std::string& default_value) {
  return default_value;
}

bool GetBoolProperty(const std::string& key, bool default_value) { return default_value; }

}  // namespace base
}  // namespace android
