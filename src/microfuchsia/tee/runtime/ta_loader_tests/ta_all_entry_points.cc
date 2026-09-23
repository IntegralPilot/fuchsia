// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

// This file exports all of the entry points that a TA is expected to export.
// The signatures for these are not correct but the exported symbol contains
// only a name so this doesn't matter for loading purposes.

extern "C" {

void __EXPORT TA_CreateEntryPoint() {}

void __EXPORT TA_DestroyEntryPoint() {}

void __EXPORT TA_OpenSessionEntryPoint() {}

void __EXPORT TA_CloseSessionEntryPoint() {}

void __EXPORT TA_InvokeCommandEntryPoint() {}

}  // extern "C"
