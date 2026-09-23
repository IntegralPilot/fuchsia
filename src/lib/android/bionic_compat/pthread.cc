// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <pthread.h>

extern "C" {

int pthread_condattr_getpshared(const pthread_condattr_t* attr, int* pshared) {
  *pshared = PTHREAD_PROCESS_PRIVATE;
  return 0;
}

int pthread_condattr_setpshared(pthread_condattr_t* attr, int pshared) {
  if (pshared == PTHREAD_PROCESS_PRIVATE) {
    return 0;
  }
  if (pshared == PTHREAD_PROCESS_SHARED) {
    return ENOTSUP;
  }
  return EINVAL;
}

int pthread_mutexattr_getpshared(const pthread_mutexattr_t* attr, int* pshared) {
  *pshared = PTHREAD_PROCESS_PRIVATE;
  return 0;
}

int pthread_mutexattr_setpshared(pthread_mutexattr_t* attr, int pshared) {
  if (pshared == PTHREAD_PROCESS_PRIVATE) {
    return 0;
  }
  if (pshared == PTHREAD_PROCESS_SHARED) {
    return ENOTSUP;
  }
  return EINVAL;
}

int pthread_rwlockattr_getpshared(const pthread_rwlockattr_t* attr, int* pshared) {
  *pshared = PTHREAD_PROCESS_PRIVATE;
  return 0;
}

int pthread_rwlockattr_setpshared(pthread_rwlockattr_t* attr, int pshared) {
  if (pshared == PTHREAD_PROCESS_PRIVATE) {
    return 0;
  }
  if (pshared == PTHREAD_PROCESS_SHARED) {
    return ENOTSUP;
  }
  return EINVAL;
}

int pthread_atfork(void (*prepare)(void), void (*parent)(void), void (*child)(void)) {
  // As fork is not available on Fuchsia, all callbacks can be ignored.
  return 0;
}
}
