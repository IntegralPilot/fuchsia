// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-module.h"

int TestStart(int x, int y) { return x + y; }

extern "C" [[gnu::visibility("default")]] const uint32_t kTestRoData = kTestRoDataValue;
