// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/sequence_checker.h>
#include <lib/async_patterns/cpp/internal/receiver_base.h>

namespace async_patterns::internal {

namespace {

constexpr const char kReceiverThreadingError[] = "|async_patterns::Receiver| is thread-unsafe.";

}

ReceiverBase::~ReceiverBase() { task_queue_->Stop(); }

async_dispatcher_t* ReceiverBase::dispatcher() const { return dispatcher_; }

ReceiverBase::ReceiverBase(async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher),
      task_queue_(TaskQueue::Create(dispatcher, kReceiverThreadingError)) {}

}  // namespace async_patterns::internal
