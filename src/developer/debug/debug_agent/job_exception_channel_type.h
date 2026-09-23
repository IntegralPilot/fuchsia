// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_JOB_EXCEPTION_CHANNEL_TYPE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_JOB_EXCEPTION_CHANNEL_TYPE_H_

namespace debug_agent {

// Specifies which exception channel on a job to claim when configuring a JobHandle attach
// via JobHandle::AttachConfig.
enum class JobExceptionChannelType {
  // Requests the "normal" job exception channel, this will report exceptions that are not handled
  // by any of the job's children to us before they are handled by the system RootJob exception
  // handler. A strong filter with the "job_only" configuration will result in using this channel
  // (corresponding to debug_ipc::AttachConfig::Priority::kStrong).
  kException,
  // Requests the "JobDebugger" exception channel, which registers us for notifications for process
  // starting events, but not exceptions. There may be many instances of this type of channel, see
  // https://fuchsia.dev/fuchsia-src/concepts/kernel/exceptions#exception_types. This setting is the
  // result of a filter with both "job_only" and "weak" configured (or when weakly monitoring a job,
  // corresponding to debug_ipc::AttachConfig::Priority::kWeak).
  kDebugger,
  // Does not request an exception channel on the job at all. Used when job monitoring is disabled
  // or for a job attach configuration with debug_ipc::AttachConfig::Priority::kMinimal.
  kNone,
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_JOB_EXCEPTION_CHANNEL_TYPE_H_
