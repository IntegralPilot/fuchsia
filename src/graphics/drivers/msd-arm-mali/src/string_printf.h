// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_STRING_PRINTF_H_
#define SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_STRING_PRINTF_H_

#include <stdarg.h>
#include <zircon/compiler.h>

#include <string>

// TODO(https://fxbug.dev/42078340): Switch to use std::format.
// Formats |printf()|-like input and returns it as an |fbl::String|.
std::string StringPrintf(const char* format, ...) __PRINTFLIKE(1, 2) __WARN_UNUSED_RESULT;

// Formats |vprintf()|-like input and returns it as an |fbl::String|.
std::string StringVPrintf(const char* format, va_list ap) __WARN_UNUSED_RESULT;

#endif  // SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_STRING_PRINTF_H_
