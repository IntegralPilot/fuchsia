// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC___SUPPORT_CPP_NEW_H_
#define PREEMPT_SRC___SUPPORT_CPP_NEW_H_

// This header has to exist basically because the adjacent tuple.h does.  Since
// that header uses <tuple>, it will also reach <new>.  But the llvm-libc
// "src/__support/CPP/new.h" conflicts with that.  Fortunately, as with tuple.h
// it's all just meant to be a namespace-renamed polyfill for what libc++
// already provides.

#include <stdlib.h>

#include <new>  // IWYU pragma: export

#include "src/__support/common.h"
#include "src/__support/macros/config.h"

#endif  // PREEMPT_SRC___SUPPORT_CPP_NEW_H_
