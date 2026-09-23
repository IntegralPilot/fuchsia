// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_FTW_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_FTW_H_

#include <sys/cdefs.h>
#include <sys/stat.h>
#include <sys/types.h>

#define FTW_F 0
#define FTW_D 1
#define FTW_DNR 2
#define FTW_DP 3
#define FTW_NS 4
#define FTW_SL 5
#define FTW_SLN 6

#define FTW_PHYS 0x01
#define FTW_MOUNT 0x02
#define FTW_DEPTH 0x04
#define FTW_CHDIR 0x08

struct FTW;
__BEGIN_DECLS
int nftw(const char* __dir_path,
         int (*__callback)(const char*, const struct stat*, int, struct FTW*), int __max_fd_count,
         int __flags);
__END_DECLS

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_FTW_H_
