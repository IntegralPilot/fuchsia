// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_SNAPSHOT_ANNOTATION_REGISTER_H_
#define SRC_SYSMEM_SERVER_SNAPSHOT_ANNOTATION_REGISTER_H_

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/sys/cpp/service_directory.h>

namespace sys {
class ServiceDirectory;
}  // namespace sys

/// Sends annotations to Feedback that are attached to its snapshots.
class SnapshotAnnotationRegister {
 public:
  explicit SnapshotAnnotationRegister(async_dispatcher_t* dispatcher);

  SnapshotAnnotationRegister(const SnapshotAnnotationRegister& other) = delete;
  SnapshotAnnotationRegister& operator=(const SnapshotAnnotationRegister& other) = delete;

  // Set the ServiceDirectory from which to get fuchsia.feedback.ComponentDataRegister.  This can
  // be nullptr. This can be called again, regardless of whether there was already a previous
  // ServiceDirectory. If not called, or if set to nullptr, no crash annotations are reported.
  // The FIDL client will be bound to the given dispatcher.
  void SetServiceDirectory(std::shared_ptr<sys::ServiceDirectory> service_directory,
                           async_dispatcher_t* dispatcher);
  void UnsetServiceDirectory() { SetServiceDirectory(nullptr, nullptr); }

  // Increments the reported number of DMA corruption events detected during the current boot.
  void IncrementNumDmaCorruptions();

 private:
  // Records what thread it is first called on, and then asserts that all subsequent calls come from
  // the same thread. We use it to ensure that `client_` is only used on the same thread in which it
  // was bound.
  void AssertRunningSynchronized();

  void Flush();

  uint64_t num_dma_corruptions_ = 0;
  fidl::Client<fuchsia_feedback::ComponentDataRegister> client_;
  async::synchronization_checker synchronization_checker_;
};

#endif  // SRC_SYSMEM_SERVER_SNAPSHOT_ANNOTATION_REGISTER_H_
