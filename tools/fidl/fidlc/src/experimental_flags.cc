// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/experimental_flags.h"

#include <lib/stdcompat/bit.h>
#include <zircon/assert.h>

namespace fidlc {

const std::map<std::string_view, ExperimentalFlag> kAllExperimentalFlags = {
    {"allow_new_types", ExperimentalFlag::kAllowNewTypes},
    {"output_index_json", ExperimentalFlag::kOutputIndexJson},
    {"zx_c_types", ExperimentalFlag::kZxCTypes},
    {"allow_arbitrary_error_types", ExperimentalFlag::kAllowArbitraryErrorTypes},
    {"no_resource_attribute", ExperimentalFlag::kNoResourceAttribute},
};

}  // namespace fidlc
