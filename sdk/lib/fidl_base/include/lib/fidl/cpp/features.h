// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_FEATURES_H_
#define LIB_FIDL_CPP_FEATURES_H_

#if __cplusplus >= 202002L
#include <version>
#endif

// __FIDL_SUPPORT_HANDLES determines whether, in the current compilation unit,
// FIDL bindings should support handles.
#if !defined(__FIDL_SUPPORT_HANDLES)
#if defined(__Fuchsia__)
#define __FIDL_SUPPORT_HANDLES 1
#else
#define __FIDL_SUPPORT_HANDLES 0
#endif
#endif

// __FIDL_SUPPORT_FORMAT determines whether, in the current compilation unit,
// FIDL bindings should support std::format.
#if !defined(__FIDL_SUPPORT_FORMAT)
#if defined(__cpp_lib_format) && __cplusplus >= 202002L && defined(__Fuchsia__)
#define __FIDL_SUPPORT_FORMAT 1
#else
#define __FIDL_SUPPORT_FORMAT 0
#endif
#endif

#endif
