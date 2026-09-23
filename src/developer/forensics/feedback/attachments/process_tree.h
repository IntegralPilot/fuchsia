// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PROCESS_TREE_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PROCESS_TREE_H_

#include <lib/zx/job.h>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"

namespace forensics::feedback {

// AttachmentProvider that generates a snapshot attachment ("process_tree.txt")
// containing a hierarchical text description of jobs, processes, and threads in Zircon
// (equivalent to `fx shell ps -T`).
class ProcessTree : public AttachmentProvider {
 public:
  // Takes ownership of a handle to a job (typically the root job) from which to walk
  // the task tree.
  explicit ProcessTree(zx::job job);

  // Returns a promise containing the process tree string table as an AttachmentData.
  // Returns Error::kMissingValue if the root job handle is invalid or tree walking fails.
  ::fpromise::promise<AttachmentData> Get(uint64_t ticket) override;

  // Completes the process tree collection promise associated with |ticket| early, if it hasn't
  // already completed (no-op as collection runs synchronously).
  void ForceCompletion(uint64_t ticket, Error error) override;

 private:
  zx::job job_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PROCESS_TREE_H_
