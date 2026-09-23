// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_PTHREAD_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_PTHREAD_H_

#include_next <pthread.h>

#define PTHREAD_PROCESS_SHARED 1

#ifdef __cplusplus
extern "C" {
#endif

int pthread_mutexattr_getpshared(const pthread_mutexattr_t*, int*);
int pthread_mutexattr_setpshared(pthread_mutexattr_t*, int);
int pthread_condattr_setpshared(pthread_condattr_t*, int);
int pthread_condattr_getpshared(const pthread_condattr_t*, int*);
int pthread_rwlockattr_setpshared(pthread_rwlockattr_t*, int);
int pthread_rwlockattr_getpshared(const pthread_rwlockattr_t*, int*);

#ifdef __cplusplus
}
#endif

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_PTHREAD_H_
