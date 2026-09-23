// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/item_location.h"

#include <string>

namespace forensics::crash_reports {

std::string ToString(const ItemLocation location) {
  switch (location) {
    case ItemLocation::kMemory:
      return "MEMORY";
    case ItemLocation::kCache:
      return "CACHE";
    case ItemLocation::kTmp:
      return "TMP";
  }
}

}  // namespace forensics::crash_reports
