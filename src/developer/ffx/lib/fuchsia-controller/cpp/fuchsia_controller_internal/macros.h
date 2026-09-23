// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_MACROS_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_MACROS_H_

#ifdef __clang_version__
#define IGNORE_EXTRA_SC _Pragma("GCC diagnostic ignored \"-Wextra-semi\"")
#else
#define IGNORE_EXTRA_SC
#endif

#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_MACROS_H_
