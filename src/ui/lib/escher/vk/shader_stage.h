// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_VK_SHADER_STAGE_H_
#define SRC_UI_LIB_ESCHER_VK_SHADER_STAGE_H_

#include <cstdint>

#include "src/ui/lib/escher/util/debug_print.h"

namespace escher {

enum class ShaderStage : uint8_t {
  kVertex = 0,
  kTessellationControl = 1,
  kTessellationEvaluation = 2,
  kGeometry = 3,
  kFragment = 4,
  kCompute = 5,
  kEnumCount
};
ESCHER_DEBUG_PRINTABLE(ShaderStage);

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_VK_SHADER_STAGE_H_
