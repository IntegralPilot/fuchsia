// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_ZYGOTE_H_
#define LIB_LD_TEST_MODULES_ZYGOTE_H_

// This is defined in the zygote-dep shared library.
extern "C" int zygote_dep();

// It uses both of these, which are defined in the zygote main executable.
extern "C" int called_by_zygote_dep();
extern int* initialized_data[];

// This is defined by the zygote main executable, but only referenced by the
// zygote-secondary module.
extern "C" int zygote_test_main();

#endif  // LIB_LD_TEST_MODULES_ZYGOTE_H_
