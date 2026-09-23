// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_KCOUNTER_PROVIDER_H_
#define LIB_KCOUNTER_PROVIDER_H_

#include <lib/svc/service.h>

const zx_service_provider_t* kcounter_get_service_provider();

#endif  // LIB_KCOUNTER_PROVIDER_H_
