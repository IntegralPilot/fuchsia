// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_MAP_MACRO_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_MAP_MACRO_H_

#include <lib/fxt/map_macro_internal.h>

//
// Utilities to map macros to argument lists in various ways.
//
// FXT_MAP(macro, a, b, c, ...)               -> macro(a) macro(b) macro(c) ...
// FXT_MAP_ARGS(macro, (a), (b, c), ...)      -> macro(a) macro(b, c) ...
// FXT_MAP_LIST(macro, a, b, c, ...)          -> macro(a), macro(b), macro(c) ...
// FXT_MAP_LIST_ARGS(macro, (a), (b, c), ...) -> macro(a), macro(b, c) ...
//

// Maps the given macro over the set of arguments.
//
// Example expansion:
//
//   FXT_MAP(FOO, a, b, c) -> FOO(a) FOO(b) FOO(c).
//
#define FXT_MAP(macro, ...) \
  FXT_INTERNAL_EXPAND(FXT_INTERNAL_MAP_START(macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0))

// Maps the given macro over the set of arguments, grouping by parentheses.
//
// Example expansion:
//
//   FXT_MAP_ARGS(FOO, a, (b, c), (d, e, f)) -> FOO(a) FOO(b, c) FOO(d, e, f).
//
#define FXT_MAP_ARGS(macro, ...) \
  FXT_INTERNAL_EXPAND(           \
      FXT_INTERNAL_MAP_ARGS_START(macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0))

// Maps the given macro over the set of arguments as a comma separated list.
//
// Example expansion:
//
//   FXT_MAP_LIST(FOO, a, b, c) -> FOO(a), FOO(b), FOO(c).
//
#define FXT_MAP_LIST(macro, ...) \
  FXT_INTERNAL_EXPAND(           \
      FXT_INTERNAL_MAP_LIST_START(macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0))

// Maps the given macro over the set of arguments as a comma separated list, grouping by
// parentheses.
//
// Example expansion:
//
//   FXT_MAP_LIST_ARGS(FOO, (a), (b, c), (d, e, f)) -> FOO(a), FOO(b, c), FOO(d, e, f).
//
#define FXT_MAP_LIST_ARGS(macro, ...) \
  FXT_INTERNAL_EXPAND(                \
      FXT_INTERNAL_MAP_LIST_ARGS_START(macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0))

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_MAP_MACRO_H_
