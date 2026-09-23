// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

// This file exports the creation entry point that TAs are expected to export.
// The signatures for this is not correct but the exported symbol contains only
// a name so this doesn't matter for loading purposes.

extern "C" {

void __EXPORT TA_CreateEntryPoint() {}

}  // extern "C"
