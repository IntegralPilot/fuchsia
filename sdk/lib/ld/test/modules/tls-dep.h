// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_TLS_DEP_H_
#define LIB_LD_TEST_MODULES_TLS_DEP_H_

#include <cstddef>

constexpr size_t kTlsDepAlign = 32;
constexpr int kTlsDepDataValue = 42;

extern "C" {

extern constinit thread_local int tls_dep_data;
extern constinit thread_local char tls_dep_bss[2];

int* get_tls_dep_data();
char* get_tls_dep_bss0();
char* get_tls_dep_bss1();
int* get_tls_dep_weak();

}  // extern "C"

// Since tls_dep_bss is what's aligned, the tls_bss_size includes the alignment
// padding between the end of .tdata (containing only tls_dep_data) and the
// aligned start of .tbss (containing tls_dep_bss).
constexpr size_t kTlsDepBssSize = kTlsDepAlign - sizeof(tls_dep_data) + sizeof(tls_dep_bss);

constexpr size_t kTlsDepTotalSize = sizeof(tls_dep_data) + kTlsDepBssSize;

constexpr size_t kTlsDepAlignedTotalSize = (kTlsDepTotalSize + kTlsDepAlign - 1) & -kTlsDepAlign;

template <class ModuleList>
inline const auto& FindTlsDep(const ModuleList& modules) {
  for (const auto& module : modules) {
    if (module.soname.str() == "libtls-dep.so") {
      return module;
    }
  }
  __builtin_trap();
}

#endif  // LIB_LD_TEST_MODULES_TLS_DEP_H_
