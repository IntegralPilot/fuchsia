// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC___SUPPORT_MACROS_ATTRIBUTES_H_
#define PREEMPT_SRC___SUPPORT_MACROS_ATTRIBUTES_H_

// TODO(https://fxbug.dev/471248163): This file exists to work around a bug in
// the RBE wrapper's C++ preprocessor logic.  It doesn't know how to resolve
// `__has_attribute` and `__has_feature`, so it thinks that macros/attribute.h
// will define LIBC_HAS_VECTOR_TYPE to 0 rather than to 1.  This leads its
// later logic to exclude `#include` files that conditionalized on things like
// `#if LIBC_HAS_VECTOR_TYPE` from its list of inputs, causing the remote
// compile to fail when it can't find all the right input files when the real
// preprocessor resolves the conditionals with `LIBC_HAS_VECTOR_TYPE` correct.

#include_next "src/__support/macros/attributes.h"

// After the real definition, override the `LIBC_HAS_VECTOR_TYPE` setting with
// simple code that the RBE wrapper's preprocessor will not misconstrue.  The
// real compiler will also do the #error check to verify the hard-coding here
// is actually correct in all configurations built for Fuchsia.

#ifdef __clang__
#define FUCHSIA_LIBC_HAS_VECTOR_TYPE 1
#else
#define FUCHSIA_LIBC_HAS_VECTOR_TYPE 0
#endif

#if __has_attribute(ext_vector_type) && __has_feature(ext_vector_type_boolean)
#if !FUCHSIA_LIBC_HAS_VECTOR_TYPE
#error "LIBC_HAS_VECTOR_TYPE should be 1"
#endif
#else
#if FUCHSIA_LIBC_HAS_VECTOR_TYPE
#error "LIBC_HAS_VECTOR_TYPE should be 0"
#endif
#endif

#undef LIBC_HAS_VECTOR_TYPE
#define LIBC_HAS_VECTOR_TYPE FUCHSIA_LIBC_HAS_VECTOR_TYPE

#endif  // PREEMPT_SRC___SUPPORT_MACROS_ATTRIBUTES_H_
