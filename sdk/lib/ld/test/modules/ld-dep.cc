// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <stdint.h>

#include "test-start.h"

extern decltype(ld::abi::_ld_abi) ld::abi::_ld_abi [[gnu::weak]];

extern "C" int64_t TestStart() { return &ld::abi::_ld_abi ? 17 : 0; }
