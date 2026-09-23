// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_TRACE_TRACE_H_
#define SRC_STORAGE_LIB_TRACE_TRACE_H_

#ifdef STORAGE_ENABLE_TRACING
#include "src/storage/lib/trace/trace_enabled.h"  // IWYU pragma: export nogncheck
#else
#include "src/storage/lib/trace/trace_disabled.h"  // IWYU pragma: export nogncheck
#endif

#endif  // SRC_STORAGE_LIB_TRACE_TRACE_H_
