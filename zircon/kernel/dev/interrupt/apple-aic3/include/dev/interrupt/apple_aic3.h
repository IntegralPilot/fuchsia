// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_INTERRUPT_APPLE_AIC3_INCLUDE_DEV_INTERRUPT_APPLE_AIC3_H_
#define ZIRCON_KERNEL_DEV_INTERRUPT_APPLE_AIC3_INCLUDE_DEV_INTERRUPT_APPLE_AIC3_H_

#include <lib/zbi-format/driver-config.h>

void AppleAic3InitEarly();
void AppleAic3HandleFiq();
void AppleAic3InitPostVm(const zbi_dcfg_apple_aic3_t& config);

#endif  // ZIRCON_KERNEL_DEV_INTERRUPT_APPLE_AIC3_INCLUDE_DEV_INTERRUPT_APPLE_AIC3_H_
