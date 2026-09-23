// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_CDEFS_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_CDEFS_H_

#if defined(__cplusplus)
#define __BEGIN_DECLS extern "C" {
#define __END_DECLS }
#else
#define __BEGIN_DECLS
#define __END_DECLS
#endif

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_CDEFS_H_
