// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_SUFFIXED_SYMBOL_H_
#define LIB_LD_TEST_MODULES_SUFFIXED_SYMBOL_H_

#ifndef TEST_SYMBOL_SUFFIX
#error "TEST_SYMBOL_SUFFIX is not defined"
#endif

#define PASTE_IMPL(a, b) a##b
#define PASTE(sym, suffix) PASTE_IMPL(sym, suffix)
#define SUFFIXED_SYMBOL(sym) PASTE(sym, TEST_SYMBOL_SUFFIX)

#endif  // LIB_LD_TEST_MODULES_SUFFIXED_SYMBOL_H_
