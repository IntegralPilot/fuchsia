// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_VIRTGRALLOC_WRAPPER_H_
#define SRC_STARNIX_LIB_VIRTGRALLOC_WRAPPER_H_

// This wrapper.h file is the root .h passed to bindgen in bindgen.sh. See
// bindgen.sh for how this file is used.

// bindgen doesn't like static_assert, so make it go away from bindgen's point
// of view
#define static_assert(x)

// This is used as the type of a const C style string global in
// missing_includes.h included below (dynamically generated during bindgen.sh).
// See bindgen.sh. Without this, bindgen (using llvm lib) complains about
// "const" repeated twice in a single-line definition of a const global (not
// valid C++), but with this typedef, the redundant const split across the
// typedef and a later usage of the typedef is valid C++. The bindgen.sh script
// removes the line defining this type from the .rs output, since it's not
// actually needed by the generated .rs code. The version of bindgen we're using
// as of this comment uses a byte array slice for the string constant, not the
// translated version of this type.
typedef const char* rust_bindgen_string;

// The actual definitions we want bindgen to translate to .rs. The #define(s) in
// this file get ignored by bindgen, but then the const globals in
// missing_includes.h (see below) are not ignored by bindgen. Those const
// globals get their values from the #defines included in this file.
#include <sdk/lib/virtgralloc/include/lib/virtgralloc/virtgralloc_ioctl.h>

// This include is dynamically generated, used, cleaned up, all during a run of
// bindgen.sh. The cleanup step in bindgen.sh can be disabled to observe/debug
// the content of this file. This file contains const definitions that bindgen
// understands how to translate, to stand in for #define constants that bindgen
// doesn't understand how to (directly) translate, with some macro stuff to get
// the names to match the #define names.
#include "missing_includes.h"

#endif  // SRC_STARNIX_LIB_VIRTGRALLOC_WRAPPER_H_
