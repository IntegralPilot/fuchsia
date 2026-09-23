// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "asm.h"

// The strchrnul implementation is verbatim from cortex-strings.

#include "third_party/lib/cortex-strings/src/aarch64/strchrnul.S"

.hidden __strchrnul
ALIAS(strchrnul, __strchrnul)
