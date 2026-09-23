// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_STRESS_RESTRICTED_BLOB_READER_H_
#define SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_STRESS_RESTRICTED_BLOB_READER_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Reads the time stored in shared_value into read_value.
void Reader(zx_instant_mono_ticks_t* shared_value, zx_instant_mono_ticks_t* read_value);

__END_CDECLS

#endif  // SRC_ZIRCON_TESTS_RESTRICTED_MODE_SHARED_STRESS_RESTRICTED_BLOB_READER_H_
