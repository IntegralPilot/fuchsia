// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/syscalls/forward.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <fbl/ref_ptr.h>
#include <object/dispatcher.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>

#define LOCAL_TRACE 0

// zx_status_t zx_object_signal
zx_status_t sys_object_signal(zx_handle_t handle_value, uint32_t clear_mask, uint32_t set_mask) {
  LTRACEF("handle %x\n", handle_value);

  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<Dispatcher> dispatcher;

  auto status =
      up->handle_table().GetDispatcherWithRights(*up, handle_value, ZX_RIGHT_SIGNAL, &dispatcher);
  if (status != ZX_OK)
    return status;

  return dispatcher->user_signal_self(clear_mask, set_mask);
}

// zx_status_t zx_object_signal_peer
zx_status_t sys_object_signal_peer(zx_handle_t handle_value, uint32_t clear_mask,
                                   uint32_t set_mask) {
  LTRACEF("handle %x\n", handle_value);

  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<Dispatcher> dispatcher;

  auto status = up->handle_table().GetDispatcherWithRights(*up, handle_value, ZX_RIGHT_SIGNAL_PEER,
                                                           &dispatcher);
  if (status != ZX_OK)
    return status;

  return dispatcher->user_signal_peer(clear_mask, set_mask);
}

// Given a kernel object with children objects, obtain a handle to the
// child specified by the provided kernel object id.
// zx_status_t zx_object_get_child
zx_status_t sys_object_get_child(zx_handle_t handle, uint64_t koid, zx_rights_t rights,
                                 zx_handle_t* out) {
  auto up = ProcessDispatcher::GetCurrent();

  fbl::RefPtr<Dispatcher> dispatcher;
  uint32_t parent_rights;
  auto status = up->handle_table().GetDispatcherAndRights(*up, handle, &dispatcher, &parent_rights);
  if (status != ZX_OK)
    return status;

  if (!(parent_rights & ZX_RIGHT_ENUMERATE))
    return ZX_ERR_ACCESS_DENIED;

  if (rights == ZX_RIGHT_SAME_RIGHTS) {
    rights = parent_rights;
  } else if ((parent_rights & rights) != rights) {
    return ZX_ERR_ACCESS_DENIED;
  }

  // TODO(https://fxbug.dev/42175105): Constructing the handles below may cause the handle count to
  // go from 0->1, resulting in multiple on_zero_handles invocations. Presently this is benign,
  // except for one scenario with processes in the initial state. Such processes are filtered out by
  // the SimpleJobEnumerator and should not be able to be learned about. Further protection against
  // guessing is not performed here since the worst case scenario is a misbehaving privileged
  // process guessing a koid and destroying a process that was in construction.
  auto process = DownCastDispatcher<ProcessDispatcher>(&dispatcher);
  if (process) {
    auto thread = process->LookupThreadById(koid);
    if (!thread)
      return ZX_ERR_NOT_FOUND;
    return up->MakeAndAddHandle(ktl::move(thread), rights, out);
  }

  auto job = DownCastDispatcher<JobDispatcher>(&dispatcher);
  if (job) {
    auto child = job->LookupJobById(koid);
    if (child)
      return up->MakeAndAddHandle(ktl::move(child), rights, out);
    auto proc = job->LookupProcessById(koid);
    if (proc) {
      return up->MakeAndAddHandle(ktl::move(proc), rights, out);
    }
    return ZX_ERR_NOT_FOUND;
  }

  return ZX_ERR_WRONG_TYPE;
}
