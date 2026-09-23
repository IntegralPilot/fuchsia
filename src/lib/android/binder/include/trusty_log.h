// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BINDER_INCLUDE_TRUSTY_LOG_H_
#define SRC_LIB_ANDROID_BINDER_INCLUDE_TRUSTY_LOG_H_

#include <android/log.h>

// Trusty log implementation delegating to __android_log_print
#define TLOGD(fmt, ...) __android_log_print(ANDROID_LOG_DEBUG, "libbinder", fmt, ##__VA_ARGS__)
#define TLOGI(fmt, ...) __android_log_print(ANDROID_LOG_INFO, "libbinder", fmt, ##__VA_ARGS__)
#define TLOGW(fmt, ...) __android_log_print(ANDROID_LOG_WARN, "libbinder", fmt, ##__VA_ARGS__)
#define TLOGE(fmt, ...) __android_log_print(ANDROID_LOG_ERROR, "libbinder", fmt, ##__VA_ARGS__)
#define TLOGC(fmt, ...) __android_log_print(ANDROID_LOG_FATAL, "libbinder", fmt, ##__VA_ARGS__)

#endif  // SRC_LIB_ANDROID_BINDER_INCLUDE_TRUSTY_LOG_H_
