// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/visitors/registration.h>

#include "lib/driver/devicetree/visitors/drivers/gpio-controllers/gpioimpl-visitor/gpioimpl-visitor.h"

REGISTER_DEVICETREE_VISITOR(gpio_impl_dt::GpioImplVisitor);
