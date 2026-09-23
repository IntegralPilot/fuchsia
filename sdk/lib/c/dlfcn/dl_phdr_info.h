// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_PHDR_INFO_H_
#define LIB_C_DLFCN_DL_PHDR_INFO_H_

// Avoid symbol conflict between <ld/abi/abi.h> and <link.h>
#pragma push_macro("_r_debug")
#undef _r_debug
#define _r_debug not_using_system_r_debug
#include <link.h>  // IWYU pragma: export
#pragma pop_macro("_r_debug")

#endif  // LIB_C_DLFCN_DL_PHDR_INFO_H_
