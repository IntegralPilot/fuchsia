// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_TLS_INITIAL_DEP_H_
#define LIB_LD_TEST_MODULES_TLS_INITIAL_DEP_H_

constexpr int kTlsInitialDepDataValue = 10;

extern "C" {

extern constinit thread_local int tls_intial_dep_data;

int* get_tls_initial_dep_data();

}  // extern "C"

#endif  // LIB_LD_TEST_MODULES_TLS_INITIAL_DEP_H_
