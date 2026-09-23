// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sysmem/server/indent.h"

IndentScope IndentTracker::Current() { return IndentScope(*this, 0); }

IndentScope IndentTracker::Nested(uint32_t level_count) { return IndentScope(*this, level_count); }
