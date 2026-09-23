// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Stubs used by third party code.

#include <cstddef>

extern "C" {
void* malloc(size_t s) { return operator new(s); }

void free(void* p) { return operator delete(p); }
}
