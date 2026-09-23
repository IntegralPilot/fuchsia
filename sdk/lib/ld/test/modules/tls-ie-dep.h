// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_TLS_IE_DEP_H_
#define LIB_LD_TEST_MODULES_TLS_IE_DEP_H_

extern "C" int* tls_ie_data();
extern "C" int* tls_ie_bss();

#endif  // LIB_LD_TEST_MODULES_TLS_IE_DEP_H_
