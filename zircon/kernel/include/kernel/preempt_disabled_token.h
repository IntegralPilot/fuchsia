// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_PREEMPT_DISABLED_TOKEN_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_PREEMPT_DISABLED_TOKEN_H_

#include <lib/concurrent/capability_token.h>

class PreemptionState;

// The preempt_disabled_token is a clang static analysis token that can be used to annotate methods
// as requiring that local preemption be disabled in order to operate properly.
//
// See kernel/auto_preempt_disabler.h for more details.
constexpr concurrent::CapabilityToken<PreemptionState> preempt_disabled_token;

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_PREEMPT_DISABLED_TOKEN_H_
