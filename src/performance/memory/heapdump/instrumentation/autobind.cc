// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <heapdump/bind.h>

namespace {
__attribute__((constructor)) void init() { heapdump_bind_with_fdio(); }
}  // namespace
