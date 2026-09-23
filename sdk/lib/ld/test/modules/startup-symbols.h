// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_STARTUP_SYMBOLS_H_
#define LIB_LD_TEST_MODULES_STARTUP_SYMBOLS_H_

#include <stdint.h>

#include <vector>

// This file contains symbols decls that are called directly by tests in
// //sdk/lib/dl/test:unittests. These symbols should be provided by targets that
// serve as startup modules for those tests.
extern "C" {

[[gnu::visibility("default")]] int64_t foo_v1_StartupModulesBasic();
[[gnu::visibility("default")]] int64_t foo_v2_StartupModulesBasic();

[[gnu::visibility("default")]] int64_t foo_v1_StartupModulesPriorityOverGlobal();
[[gnu::visibility("default")]] int64_t call_foo_v1_StartupModulesPriorityOverGlobal();

[[gnu::visibility("default")]] int64_t call_foo_v1_StartupModulesDep();

[[gnu::visibility("default")]] int* get_static_tls_var();

[[gnu::visibility("default"),
  gnu::tls_model("global-dynamic")]] extern constinit thread_local int gStaticTlsVar;
}

// This class interface that can be called by test modules to execute a Callback
// method defined by tests in //sdk/lib/dl/test/dl-load-tests-initfini.cc
class TestCallback {
 public:
  // This method takes an identity value for code in test modules to pass to
  // signify the code in that module as run.
  virtual void Callback(int) = 0;
};

// Tests set this global to an instantiated object that defines the Callback
// method.
[[gnu::visibility("default")]] extern TestCallback* gTestCallback;

constexpr int kStaticTlsDataValue = 16;

#endif  // LIB_LD_TEST_MODULES_STARTUP_SYMBOLS_H_
