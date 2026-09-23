// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <dev/iommu/bti.h>
#include <object/process_dispatcher.h>
#include <object/root_job_observer.h>
#include <object/thread_dispatcher.h>

namespace iommu {

void Bti::PrintQuarantineWarning(BtiPageLeakReason reason, uint64_t total_leaked_pages,
                                 size_t total_leaked_vmos) {
  char proc_name[ZX_MAX_NAME_LEN] = {0};
  char thread_name[ZX_MAX_NAME_LEN] = {0};
  char bti_name[ZX_MAX_NAME_LEN] = {0};

  // If we have no current thread dispatcher, then this is a kernel thread.  We
  // have no process to report, just report that the action was taken by a
  // kernel thread and leave it at that.
  ThreadDispatcher* thread_disp = ThreadDispatcher::GetCurrent();
  if (thread_disp == nullptr) {
    snprintf(proc_name, sizeof(proc_name), "<kernel>");
    snprintf(thread_name, sizeof(thread_name), "<kernel>");
  } else {
    // Get the name of the user mode process and thread which closed the handle
    // to the object which eventually resulted in the leak.
    zx_status_t status = ProcessDispatcher::GetCurrent()->get_name(proc_name);
    DEBUG_ASSERT(status == ZX_OK);

    status = thread_disp->get_name(thread_name);
    if (status != ZX_OK) {
      snprintf(thread_name, sizeof(thread_name), "<koid %lu>", thread_disp->get_koid());
    }
  }

  // Fetch the BTI name (if any).
  [[maybe_unused]] zx_status_t status = this->get_name(bti_name);
  DEBUG_ASSERT(status == ZX_OK);

  // If any of these strings are empty, replace them with just "<unknown>".
  if (!proc_name[0]) {
    snprintf(proc_name, sizeof(proc_name), "<unknown>");
  }
  if (!thread_name[0]) {
    snprintf(thread_name, sizeof(thread_name), "<unknown>");
  }
  if (!bti_name[0]) {
    snprintf(bti_name, sizeof(bti_name), "<unknown>");
  }

  // Finally, print the message describing the leak, as best we can.
  const char* leak_cause;
  switch (reason) {
    case BtiPageLeakReason::BtiOrphanedWithQuarantinedPmts:
      leak_cause = "a BTI having it's final handle closed with a non-empty quarantine list";
      break;

    case BtiPageLeakReason::PmtQuarantinedWhenBtiOrphaned:
      leak_cause =
          "a pinned PMT having its final handle closed after its BTI also had its final handle "
          "closed";
      break;

    case BtiPageLeakReason::PmtQuarantined:
      leak_cause = "a pinned PMT having its final handle closed without being unpinned first";
      break;

    default:
      leak_cause = "<unknown>";
      break;
  }

  // If we're being torn down for reasons other than the root job being killed
  // over a critical process dying then fire off a DRIVER OOPS to flag the
  // improper handling of pinned pages.
  if (!RootJobObserver::GetCriticalProcessDying()) {
    if (reason != BtiPageLeakReason::PmtQuarantined) {
      DRIVER_OOPS(
          "Bus Transaction Initiator (ID 0x%lx, name \"%s\") has leaked %" PRIu64
          " pages in %zu VMOs. Leak was caused by %s. The last handle was closed by process "
          "\"%s\", and thread \"%s\"\n",
          bti_id(), bti_name, total_leaked_pages, total_leaked_vmos, leak_cause, proc_name,
          thread_name);
    } else {
      DRIVER_WARN(
          "Bus Transaction Initiator (ID 0x%lx, name \"%s\") has leaked %" PRIu64
          " pages in %zu VMOs. Leak was caused by %s. The last handle was closed by process "
          "\"%s\", and thread \"%s\"\n",
          bti_id(), bti_name, total_leaked_pages, total_leaked_vmos, leak_cause, proc_name,
          thread_name);
    }
  }
}

}  // namespace iommu
