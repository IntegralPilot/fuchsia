// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_TLS_LD_DEP_H_
#define LIB_LD_TEST_MODULES_TLS_LD_DEP_H_

#include <cstddef>

constexpr size_t kTlsDepAlign = 32;
constexpr int kTlsLdDepDataValue = 23;
constexpr ptrdiff_t kTlsLdDepBss1Offset = kTlsDepAlign + 1;

extern "C" {

int* get_tls_ld_dep_data();

char* get_tls_ld_dep_bss0();

char* get_tls_ld_dep_bss1();

}  // extern "C"

#endif  // LIB_LD_TEST_MODULES_TLS_LD_DEP_H_
