// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/intrin.h>
#include <lib/concurrent/seqlock.inc.h>
#include <lib/kconcurrent/seqlock.h>

#include <platform/timer.h>

// Manually expand the SeqLock templates using the Fuchsia user-mode OSAL
template class ::concurrent::internal::SeqLock<::internal::FuchsiaKernelOsal,
                                               ::concurrent::SyncOpt::AcqRelOps>;
template class ::concurrent::internal::SeqLock<::internal::FuchsiaKernelOsal,
                                               ::concurrent::SyncOpt::Fence>;
