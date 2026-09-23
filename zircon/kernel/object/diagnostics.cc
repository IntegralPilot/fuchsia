// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/diagnostics.h"

#include <inttypes.h>
#include <lib/console.h>
#include <lib/ktrace.h>
#include <stdio.h>
#include <string.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <kernel/deadline.h>
#include <ktl/span.h>
#include <object/fifo_dispatcher.h>
#include <object/handle.h>
#include <object/io_buffer_dispatcher.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/socket_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <pretty/cpp/sizes.h>
#include <vm/fault.h>

#include <ktl/enforce.h>

namespace {

using pretty::FormattedBytes;

// Machinery to walk over a job tree and run a callback on each process.
template <typename ProcessCallbackType>
class ProcessWalker final : public JobEnumerator {
 public:
  ProcessWalker(ProcessCallbackType cb) : cb_(cb) {}
  ProcessWalker(const ProcessWalker&) = delete;
  ProcessWalker(ProcessWalker&& other) : cb_(other.cb_) {}

 private:
  bool OnProcess(ProcessDispatcher* process) final {
    cb_(process);
    return true;
  }

  const ProcessCallbackType cb_;
};

template <typename ProcessCallbackType>
ProcessWalker<ProcessCallbackType> MakeProcessWalker(ProcessCallbackType cb) {
  return ProcessWalker<ProcessCallbackType>(cb);
}

// Machinery to walk over a job tree and run a callback on each job.
template <typename JobCallbackType>
class JobWalker final : public JobEnumerator {
 public:
  JobWalker(JobCallbackType cb) : cb_(cb) {}
  JobWalker(const JobWalker&) = delete;
  JobWalker(JobWalker&& other) : cb_(other.cb_) {}

 private:
  bool OnJob(JobDispatcher* job) final {
    cb_(job);
    return true;
  }

  const JobCallbackType cb_;
};

template <typename JobCallbackType>
JobWalker<JobCallbackType> MakeJobWalker(JobCallbackType cb) {
  return JobWalker<JobCallbackType>(cb);
}

void DumpProcessListKeyMap() {
  printf("id  : process id number\n");
  printf("#h  : total number of handles\n");
  printf("#jb : number of job handles\n");
  printf("#pr : number of process handles\n");
  printf("#th : number of thread handles\n");
  printf("#vo : number of vmo handles\n");
  printf("#vm : number of virtual memory address region handles\n");
  printf("#ch : number of channel handles\n");
  printf("#ev : number of event and event pair handles\n");
  printf("#po : number of port handles\n");
  printf("#so: number of sockets\n");
  printf("#tm : number of timers\n");
  printf("#fi : number of fifos\n");
  printf("#?? : number of all other handle types\n");
}

const char* ObjectTypeToString(zx_obj_type_t type) {
  switch (type) {
    case ZX_OBJ_TYPE_PROCESS:
      return "process";
    case ZX_OBJ_TYPE_THREAD:
      return "thread";
    case ZX_OBJ_TYPE_VMO:
      return "vmo";
    case ZX_OBJ_TYPE_CHANNEL:
      return "channel";
    case ZX_OBJ_TYPE_EVENT:
      return "event";
    case ZX_OBJ_TYPE_PORT:
      return "port";
    case ZX_OBJ_TYPE_INTERRUPT:
      return "interrupt";
    case ZX_OBJ_TYPE_DEBUGLOG:
      return "log";
    case ZX_OBJ_TYPE_SOCKET:
      return "socket";
    case ZX_OBJ_TYPE_RESOURCE:
      return "resource";
    case ZX_OBJ_TYPE_EVENTPAIR:
      return "event-pair";
    case ZX_OBJ_TYPE_JOB:
      return "job";
    case ZX_OBJ_TYPE_VMAR:
      return "vmar";
    case ZX_OBJ_TYPE_FIFO:
      return "fifo";
    case ZX_OBJ_TYPE_GUEST:
      return "guest";
    case ZX_OBJ_TYPE_VCPU:
      return "vcpu";
    case ZX_OBJ_TYPE_TIMER:
      return "timer";
    case ZX_OBJ_TYPE_IOMMU:
      return "iommu";
    case ZX_OBJ_TYPE_BTI:
      return "bti";
    case ZX_OBJ_TYPE_PROFILE:
      return "profile";
    case ZX_OBJ_TYPE_PMT:
      return "pmt";
    case ZX_OBJ_TYPE_SUSPEND_TOKEN:
      return "suspend-token";
    case ZX_OBJ_TYPE_PAGER:
      return "pager";
    case ZX_OBJ_TYPE_EXCEPTION:
      return "exception";
    case ZX_OBJ_TYPE_CLOCK:
      return "clock";
    case ZX_OBJ_TYPE_STREAM:
      return "stream";
    case ZX_OBJ_TYPE_MSI:
      return "msi";
    case ZX_OBJ_TYPE_IOB:
      return "iob";
    case ZX_OBJ_TYPE_COUNTER:
      return "counter";
    case ZX_OBJ_TYPE_IOB_SHARED_REGION:
      return "iob-shared-region";
    case ZX_OBJ_TYPE_SAMPLER:
      return "sampler";
    default:
      return "???";
  }
}

// Returns the count of a process's handles. For each handle, the corresponding
// zx_obj_type_t-indexed element of |handle_types| is incremented.
using HandleTypeCounts = ktl::span<uint32_t, ZX_OBJ_TYPE_UPPER_BOUND>;
uint32_t BuildHandleStats(const ProcessDispatcher& pd, HandleTypeCounts handle_types) {
  uint32_t total = 0;
  pd.handle_table().ForEachHandle(
      [&](zx_handle_t handle, zx_rights_t rights, const Dispatcher* disp) {
        uint32_t type = static_cast<uint32_t>(disp->get_type());
        ++handle_types[type];
        ++total;
        return ZX_OK;
      });
  return total;
}

// Counts the process's handles by type and formats them into the provided
// buffer as strings.
void FormatHandleTypeCount(const ProcessDispatcher& pd, char* buf, size_t buf_len) {
  uint32_t types[ZX_OBJ_TYPE_UPPER_BOUND] = {0};
  uint32_t handle_count = BuildHandleStats(pd, types);

  snprintf(buf, buf_len, "%4u: %4u %3u %3u %3u %3u %3u %3u %3u %3u %3u %3u %3u", handle_count,
           types[ZX_OBJ_TYPE_JOB], types[ZX_OBJ_TYPE_PROCESS], types[ZX_OBJ_TYPE_THREAD],
           types[ZX_OBJ_TYPE_VMO], types[ZX_OBJ_TYPE_VMAR], types[ZX_OBJ_TYPE_CHANNEL],
           types[ZX_OBJ_TYPE_EVENT] + types[ZX_OBJ_TYPE_EVENTPAIR], types[ZX_OBJ_TYPE_PORT],
           types[ZX_OBJ_TYPE_SOCKET], types[ZX_OBJ_TYPE_TIMER], types[ZX_OBJ_TYPE_FIFO],
           types[ZX_OBJ_TYPE_INTERRUPT] + types[ZX_OBJ_TYPE_LOG] + types[ZX_OBJ_TYPE_RESOURCE] +
               types[ZX_OBJ_TYPE_GUEST] + types[ZX_OBJ_TYPE_VCPU] + types[ZX_OBJ_TYPE_IOMMU] +
               types[ZX_OBJ_TYPE_BTI] + types[ZX_OBJ_TYPE_PROFILE] + types[ZX_OBJ_TYPE_PMT] +
               types[ZX_OBJ_TYPE_SUSPEND_TOKEN] + types[ZX_OBJ_TYPE_PAGER] +
               types[ZX_OBJ_TYPE_EXCEPTION]);
}

void DumpProcessList() {
  printf("%7s   #h:  #jb #pr #th #vo #vm #ch #ev #po #so #tm #fi #?? [name]\n", "id");

  auto walker = MakeProcessWalker([](ProcessDispatcher* process) {
    char handle_counts[(ZX_OBJ_TYPE_UPPER_BOUND * 4) + 1 + /*slop*/ 16];
    FormatHandleTypeCount(*process, handle_counts, sizeof(handle_counts));

    char pname[ZX_MAX_NAME_LEN];
    [[maybe_unused]] zx_status_t status = process->get_name(pname);
    DEBUG_ASSERT(status == ZX_OK);
    printf("%7" PRIu64 " %s [%s]\n", process->get_koid(), handle_counts, pname);
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
}

void DumpJobList() {
  printf("All jobs:\n");
  printf("%7s %s\n", "koid", "name");
  auto walker = MakeJobWalker([](JobDispatcher* job) {
    char name[ZX_MAX_NAME_LEN];
    [[maybe_unused]] zx_status_t status = job->get_name(name);
    DEBUG_ASSERT(status == ZX_OK);
    printf("%7" PRIu64 " '%s'\n", job->get_koid(), name);
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
}

void DumpPeerInfo(zx_obj_type_t type, const Dispatcher* disp) {
  const zx_koid_t koid = disp->get_koid();
  const zx_koid_t peer_koid = disp->get_related_koid();

  switch (type) {
    case ZX_OBJ_TYPE_CHANNEL: {
      auto chan = DownCastDispatcher<const ChannelDispatcher>(disp);
      const ChannelDispatcher::MessageCounts counts = chan->get_message_counts();

      printf("    chan %7" PRIu64 " %7" PRIu64 " count %" PRIu64 " max %" PRIu64 "\n", koid,
             peer_koid, counts.current, counts.max);
      break;
    }
    case ZX_OBJ_TYPE_SOCKET: {
      auto sock = DownCastDispatcher<const SocketDispatcher>(disp);
      const zx_info_socket_t sock_info = sock->GetInfo();
      const uint32_t flags = sock_info.options;

      const char* sock_type = (flags & ZX_SOCKET_STREAM) ? "stream\0" : "datagram\0";
      printf("    sock %s %7" PRIu64 " %7" PRIu64 " buf_avail %" PRIu64 "\n", sock_type, koid,
             peer_koid, sock_info.rx_buf_available);
      break;
    }

    case ZX_OBJ_TYPE_FIFO: {
      printf("    fifo %7" PRIu64 " %7" PRIu64 "\n", koid, peer_koid);
      break;
    }
    case ZX_OBJ_TYPE_EVENTPAIR: {
      printf("    eventpair %7" PRIu64 " %7" PRIu64 "\n", koid, peer_koid);
      break;
    }
    case ZX_OBJ_TYPE_IOB: {
      auto iobuf = DownCastDispatcher<const IoBufferDispatcher>(disp);

      auto region_count = iobuf->RegionCount();
      printf("    iob %7" PRIu64 " %7" PRIu64 " region count %" PRIu64 "\n", koid, peer_koid,
             region_count);
      break;
    }

    default: {
      printf("Unexpected error, peer type not supported.\n");
      break;
    }
  }
}

typedef void (*dump_peer_info)(const Dispatcher*);

void DumpProcessPeerDispatchers(zx_obj_type_t type, fbl::RefPtr<ProcessDispatcher> process,
                                zx_koid_t koid_filter = ZX_KOID_INVALID) {
  char pname[ZX_MAX_NAME_LEN];
  bool printed_header = false;

  process->handle_table().ForEachHandle(
      [&](zx_handle_t handle, zx_rights_t rights, const Dispatcher* disp) {
        if (disp->get_type() == type) {
          zx_koid_t koid = disp->get_koid();
          zx_koid_t peer_koid = disp->get_related_koid();

          if (koid_filter != ZX_KOID_INVALID && koid_filter != koid && koid_filter != peer_koid) {
            return ZX_OK;
          }

          if (!printed_header) {
            [[maybe_unused]] zx_status_t status = process->get_name(pname);
            DEBUG_ASSERT(status == ZX_OK);
            printf("%7" PRIu64 " [%s]\n", process->get_koid(), pname);
            printed_header = true;
          }

          DumpPeerInfo(type, disp);
        }

        return ZX_OK;
      });
}

void DumpPeerDispatchersByKoid(zx_obj_type_t type, zx_koid_t id) {
  auto pd = ProcessDispatcher::LookupProcessById(id);

  if (pd) {
    DumpProcessPeerDispatchers(type, pd);
  } else {
    auto walker = MakeProcessWalker([id, type](ProcessDispatcher* process) {
      DumpProcessPeerDispatchers(type, fbl::RefPtr(process), id);
    });
    GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
  }
}

void DumpAllPeerDispatchers(zx_obj_type_t type) {
  auto walker = MakeProcessWalker([type](ProcessDispatcher* process) {
    DumpProcessPeerDispatchers(type, fbl::RefPtr(process));
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
}

const char kRightsHeader[] =
    "dup tr r w x map gpr spr enm des spo gpo sig sigp wt ins mj mp mt ap ms";
void DumpHandleRightsKeyMap() {
  printf("dup : ZX_RIGHT_DUPLICATE\n");
  printf("tr  : ZX_RIGHT_TRANSFER\n");
  printf("r   : ZX_RIGHT_READ\n");
  printf("w   : ZX_RIGHT_WRITE\n");
  printf("x   : ZX_RIGHT_EXECUTE\n");
  printf("map : ZX_RIGHT_MAP\n");
  printf("gpr : ZX_RIGHT_GET_PROPERTY\n");
  printf("spr : ZX_RIGHT_SET_PROPERTY\n");
  printf("enm : ZX_RIGHT_ENUMERATE\n");
  printf("des : ZX_RIGHT_DESTROY\n");
  printf("spo : ZX_RIGHT_SET_POLICY\n");
  printf("gpo : ZX_RIGHT_GET_POLICY\n");
  printf("sig : ZX_RIGHT_SIGNAL\n");
  printf("sigp: ZX_RIGHT_SIGNAL_PEER\n");
  printf("wt  : ZX_RIGHT_WAIT\n");
  printf("ins : ZX_RIGHT_INSPECT\n");
  printf("mj  : ZX_RIGHT_MANAGE_JOB\n");
  printf("mp  : ZX_RIGHT_MANAGE_PROCESS\n");
  printf("mt  : ZX_RIGHT_MANAGE_THREAD\n");
  printf("ap  : ZX_RIGHT_APPLY_PROFILE\n");
  printf("ms  : ZX_RIGHT_MANAGE_SOCKET\n");
}

bool HasRights(zx_rights_t rights, zx_rights_t desired) { return (rights & desired) == desired; }

void FormatHandleRightsMask(zx_rights_t rights, char* buf, size_t buf_len) {
  snprintf(buf, buf_len,
           "%3d %2d %1d %1d %1d %3d %3d %3d %3d %3d %3d %3d %3d %4d %2d %3d %2d %2d %2d %2d %2d",
           HasRights(rights, ZX_RIGHT_DUPLICATE), HasRights(rights, ZX_RIGHT_TRANSFER),
           HasRights(rights, ZX_RIGHT_READ), HasRights(rights, ZX_RIGHT_WRITE),
           HasRights(rights, ZX_RIGHT_EXECUTE), HasRights(rights, ZX_RIGHT_MAP),
           HasRights(rights, ZX_RIGHT_GET_PROPERTY), HasRights(rights, ZX_RIGHT_SET_PROPERTY),
           HasRights(rights, ZX_RIGHT_ENUMERATE), HasRights(rights, ZX_RIGHT_DESTROY),
           HasRights(rights, ZX_RIGHT_SET_POLICY), HasRights(rights, ZX_RIGHT_GET_POLICY),
           HasRights(rights, ZX_RIGHT_SIGNAL), HasRights(rights, ZX_RIGHT_SIGNAL_PEER),
           HasRights(rights, ZX_RIGHT_WAIT), HasRights(rights, ZX_RIGHT_INSPECT),
           HasRights(rights, ZX_RIGHT_MANAGE_JOB), HasRights(rights, ZX_RIGHT_MANAGE_PROCESS),
           HasRights(rights, ZX_RIGHT_MANAGE_THREAD), HasRights(rights, ZX_RIGHT_APPLY_PROFILE),
           HasRights(rights, ZX_RIGHT_MANAGE_SOCKET));
}

struct JobPolicyNameValue {
  const char* name;
  uint32_t value;
};

#define ENTRY(x) \
  JobPolicyNameValue { #x, x }

constexpr ktl::array<JobPolicyNameValue, ZX_POL_MAX> kJobPolicies = {
    ENTRY(ZX_POL_BAD_HANDLE),  ENTRY(ZX_POL_WRONG_OBJECT),
    ENTRY(ZX_POL_VMAR_WX),     ENTRY(ZX_POL_NEW_ANY),
    ENTRY(ZX_POL_NEW_VMO),     ENTRY(ZX_POL_NEW_CHANNEL),
    ENTRY(ZX_POL_NEW_EVENT),   ENTRY(ZX_POL_NEW_EVENTPAIR),
    ENTRY(ZX_POL_NEW_PORT),    ENTRY(ZX_POL_NEW_SOCKET),
    ENTRY(ZX_POL_NEW_FIFO),    ENTRY(ZX_POL_NEW_TIMER),
    ENTRY(ZX_POL_NEW_PROCESS), ENTRY(ZX_POL_NEW_PROFILE),
    ENTRY(ZX_POL_NEW_PAGER),   ENTRY(ZX_POL_AMBIENT_MARK_VMO_EXEC),
    ENTRY(ZX_POL_NEW_IOB),     ENTRY(ZX_POL_NEW_SAMPLER),
};

static_assert(kJobPolicies.size() == ZX_POL_MAX, "Missing Policy Id");

const char* PolicyActionToString(uint32_t action) {
  if (action >= ZX_POL_ACTION_MAX) {
    return "INVALID ACTION";
  }

  switch (action) {
    case ZX_POL_ACTION_ALLOW:
      return "Allow";
    case ZX_POL_ACTION_DENY:
      return "Deny";
    case ZX_POL_ACTION_ALLOW_EXCEPTION:
      return "Allow+Exception";
    case ZX_POL_ACTION_DENY_EXCEPTION:
      return "Deny+Exception";
    case ZX_POL_ACTION_KILL:
      return "Kill";
    default:
      return "Unknown";
  }
}

const char* PolicyOverrideToString(uint32_t override_val) {
  switch (override_val) {
    case ZX_POL_OVERRIDE_ALLOW:
      return "Allow override";
    case ZX_POL_OVERRIDE_DENY:
      return "Deny override";
    default:
      return "Unknown";
  }
}

const char* SlackModeToString(slack_mode mode) {
  switch (mode) {
    case TIMER_SLACK_CENTER:
      return "TIMER_SLACK_CENTER";
    case TIMER_SLACK_EARLY:
      return "TIMER_SLACK_EARLY";
    case TIMER_SLACK_LATE:
      return "TIMER_SLACK_LATE";
    default:
      return "Unknown";
  }
}

void DumpJobPolicies(JobDispatcher* job) {
  char jname[ZX_MAX_NAME_LEN] = {0};
  [[maybe_unused]] zx_status_t status = job->get_name(jname);
  DEBUG_ASSERT(status == ZX_OK);
  printf("job %" PRIu64 " ('%s') Basic Policies:\n", job->get_koid(), jname);
  printf("%-30s\t%-15s\t%-15s\n", "Policy", "Action", "Override");

  JobPolicy policy = job->GetPolicy();

  for (size_t i = 0; i < kJobPolicies.size(); i++) {
    uint32_t action = policy.QueryBasicPolicy(kJobPolicies[i].value);
    uint32_t policy_override = policy.QueryBasicPolicyOverride(kJobPolicies[i].value);

    printf("%-30s\t%-15s\t%-15s\n", kJobPolicies[i].name, PolicyActionToString(action),
           PolicyOverrideToString(policy_override));
  }

  printf("Slack Policy:\n");
  const TimerSlack slack = policy.GetTimerSlack();
  printf("mode: %s\n", SlackModeToString(slack.mode()));
  printf("duration: %" PRId64 "ns\n", slack.amount());
}

void DumpJobPolicies(zx_koid_t id) {
  auto walker = MakeJobWalker([id](JobDispatcher* job) {
    if (job->get_koid() != id) {
      return;
    }

    DumpJobPolicies(job);
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
}

void DumpProcessHandles(zx_koid_t id) {
  auto pd = ProcessDispatcher::LookupProcessById(id);
  if (!pd) {
    printf("process %" PRIu64 " not found!\n", id);
    return;
  }

  char pname[ZX_MAX_NAME_LEN];
  [[maybe_unused]] zx_status_t status = pd->get_name(pname);
  DEBUG_ASSERT(status == ZX_OK);
  printf("process %" PRIu64 " ('%s') handles:\n", id, pname);
  printf("%7s %10s %10s: {%s} [type]\n", "koid", "handle", "rights", kRightsHeader);

  uint32_t total = 0;
  pd->handle_table().ForEachHandle(
      [&](zx_handle_t handle, zx_rights_t rights, const Dispatcher* disp) {
        char rights_mask[sizeof(kRightsHeader)];
        FormatHandleRightsMask(rights, rights_mask, sizeof(rights_mask));
        printf("%7" PRIu64 " %#10x %#10x: {%s} [%s]\n", disp->get_koid(), handle, rights,
               rights_mask, ObjectTypeToString(disp->get_type()));
        ++total;
        return ZX_OK;
      });
  printf("total: %u handles\n", total);
}

}  // namespace

void DumpHandlesForKoid(zx_koid_t id) {
  if (id < ZX_KOID_FIRST) {
    printf("invalid koid, non-reserved koids start at %" PRIu64 "\n", ZX_KOID_FIRST);
    return;
  }

  // Wrapper type to let us build a linked list of all child processes.
  struct ProcessNode : fbl::SinglyLinkedListable<ktl::unique_ptr<ProcessNode>> {
    fbl::RefPtr<ProcessDispatcher> proc;
  };
  fbl::SinglyLinkedList<ktl::unique_ptr<ProcessNode>> procs;

  bool alloc_failed = false;
  // Enumerate all the children and build the linked list of all processes. This lets us separate
  // the enumeration of the children, which requires holding the job dispatcher lock, with the
  // iteration of the handles, which requires holding the handle table lock. Specifically it ensures
  // we do not take the locks in the order JobDispatcher->HandleTable, as this is invalid.
  auto find_processes = MakeProcessWalker([&](ProcessDispatcher* process) {
    fbl::AllocChecker ac;
    ktl::unique_ptr<ProcessNode> p = ktl::make_unique<ProcessNode>(&ac);
    if (ac.check()) {
      p->proc = fbl::RefPtr(process);
      procs.push_front(ktl::move(p));
    } else {
      alloc_failed = true;
    }
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&find_processes);

  if (alloc_failed) {
    printf("WARNING: Allocation failure, search has been truncated\n");
  }

  uint32_t total_proc = 0;
  uint32_t total_handles = 0;

  // Inspect all the handle tables of the processes we previously found.
  for (auto& proc : procs) {
    bool found_handle = false;
    proc.proc->handle_table().ForEachHandle([&](zx_handle_t handle, zx_rights_t rights,
                                                const Dispatcher* disp) {
      if (disp->get_koid() != id) {
        return ZX_OK;
      }

      if (total_handles == 0) {
        printf("handles for koid %" PRIu64 " (%s):\n", id, ObjectTypeToString(disp->get_type()));
        printf("%7s %10s: {%s} [name]\n", "pid", "rights", kRightsHeader);
      }

      char pname[ZX_MAX_NAME_LEN];
      char rights_mask[sizeof(kRightsHeader)];
      [[maybe_unused]] zx_status_t status = proc.proc->get_name(pname);
      DEBUG_ASSERT(status == ZX_OK);
      FormatHandleRightsMask(rights, rights_mask, sizeof(rights_mask));
      printf("%7" PRIu64 " %#10x: {%s} [%s]\n", proc.proc->get_koid(), rights, rights_mask, pname);

      ++total_handles;
      found_handle = true;
      return ZX_OK;
    });
    total_proc += found_handle;
  }
  if (total_handles > 0) {
    printf("total: %u handles in %u processes\n", total_handles, total_proc);
  } else {
    printf("no handles found for koid %" PRIu64 "\n", id);
  }
}

void ktrace_report_live_processes() {
  // PID 0 refers to the kernel.
  KTRACE_KERNEL_OBJECT_ALWAYS(/* koid */ 0, ZX_OBJ_TYPE_PROCESS, "kernel",
                              ("job", ktrace::Koid(0)));

  auto walker = MakeProcessWalker([](ProcessDispatcher* process) {
    char name[ZX_MAX_NAME_LEN];
    [[maybe_unused]] zx_status_t status = process->get_name(name);
    DEBUG_ASSERT(status == ZX_OK);
    KTRACE_KERNEL_OBJECT_ALWAYS(process->get_koid(), ZX_OBJ_TYPE_PROCESS, name,
                                ("job", ktrace::Koid(process->job()->get_koid())));
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
}

namespace {

// Returns a string representation of VMO-related rights.
constexpr size_t kRightsStrLen = 8;
const char* VmoRightsToString(uint32_t rights, char str[kRightsStrLen]) {
  char* c = str;
  *c++ = (rights & ZX_RIGHT_READ) ? 'r' : '-';
  *c++ = (rights & ZX_RIGHT_WRITE) ? 'w' : '-';
  *c++ = (rights & ZX_RIGHT_EXECUTE) ? 'x' : '-';
  *c++ = (rights & ZX_RIGHT_MAP) ? 'm' : '-';
  *c++ = (rights & ZX_RIGHT_DUPLICATE) ? 'd' : '-';
  *c++ = (rights & ZX_RIGHT_TRANSFER) ? 't' : '-';
  *c = '\0';
  return str;
}

// Prints a header for the columns printed by DumpVmObject.
// If |handles| is true, the dumped objects are expected to have handle info.
void PrintVmoDumpHeader(bool handles) {
  printf("%s koid obj                parent #depth #chld #map #shr    size   uncomp   comp name\n",
         handles ? "      handle rights " : "           -      - ");
}

void DumpVmObject(const VmObject& vmo, pretty::SizeUnit format_unit, zx_handle_t handle,
                  uint32_t rights, zx_koid_t koid) {
  char handle_str[11];
  if (handle != ZX_HANDLE_INVALID) {
    snprintf(handle_str, sizeof(handle_str), "%u", static_cast<uint32_t>(handle));
  } else {
    handle_str[0] = '-';
    handle_str[1] = '\0';
  }

  char rights_str[kRightsStrLen];
  if (rights != 0) {
    VmoRightsToString(rights, rights_str);
  } else {
    rights_str[0] = '-';
    rights_str[1] = '\0';
  }

  FormattedBytes size_str(vmo.size(), format_unit);

  FormattedBytes uncomp_size;
  FormattedBytes comp_size;
  const char* uncomp_str = "phys";
  const char* comp_str = "phys";
  if (vmo.is_paged()) {
    VmObject::AttributionCounts counts = vmo.GetAttributedMemory();
    uncomp_size.SetSize(counts.uncompressed_bytes, format_unit);
    comp_size.SetSize(counts.compressed_bytes, format_unit);
    uncomp_str = uncomp_size.c_str();
    comp_str = comp_size.c_str();
  }

  char child_str[21];
  if (vmo.child_type() != VmObject::kNotChild) {
    snprintf(child_str, sizeof(child_str), "%" PRIu64, vmo.parent_user_id());
  } else {
    child_str[0] = '-';
    child_str[1] = '\0';
  }

  char name[ZX_MAX_NAME_LEN];
  vmo.get_name(name, sizeof(name));
  if (name[0] == '\0') {
    name[0] = '-';
    name[1] = '\0';
  }

  printf(
      "  %10s "  // handle
      "%6s "     // rights
      "%5" PRIu64
      " "     // koid
      "%p "   // vm_object
      "%6s "  // child parent koid
      "%6" PRIu32
      " "  // lookup depth
      "%5" PRIu32
      " "  // number of children
      "%4" PRIu32
      " "  // map count
      "%4" PRIu32
      " "      // share count
      "%7s "   // size in bytes
      "%7s "   // uncompressed bytes
      "%7s "   // compressed bytes
      "%s\n",  // name
      handle_str, rights_str, koid, &vmo, child_str, vmo.DebugLookupDepth(), vmo.num_children(),
      vmo.num_mappings(), vmo.share_count(), size_str.c_str(), uncomp_str, comp_str, name);
}

// If |hidden_only| is set, will only dump VMOs that are not mapped
// into any process:
// - VMOs that userspace has handles to but does not map
// - VMOs that are mapped only into kernel space
// - Kernel-only, unmapped VMOs that have no handles
void DumpAllVmObjects(bool hidden_only, pretty::SizeUnit format_unit) {
  if (hidden_only) {
    printf("\"Hidden\" VMOs, oldest to newest:\n");
  } else {
    printf("All VMOs, oldest to newest:\n");
  }
  PrintVmoDumpHeader(/* handles */ false);
  VmObject::ForEach([=](const VmObject& vmo) {
    if (hidden_only && vmo.IsMappedByUser()) {
      return ZX_OK;
    }
    DumpVmObject(vmo, format_unit, ZX_HANDLE_INVALID,
                 /* rights */ 0u,
                 /* koid */ vmo.user_id());
    // TODO(dbort): Dump the VmAspaces (processes) that map the VMO.
    // TODO(dbort): Dump the processes that hold handles to the VMO.
    //     This will be a lot harder to gather.
    return ZX_OK;
  });
  PrintVmoDumpHeader(/* handles */ false);
}

// Dump memory held by slices whose parent VMOs have gone away.
void DumpOrphanedSlices(pretty::SizeUnit format_unit) {
  size_t orphaned = 0;
  size_t total = 0;
  size_t contig = 0, pager = 0, anon = 0;
  size_t orphaned_bytes = 0;
  VmObject::ForEachRef([&](VmObject* vmo) {
    if (auto paged = DownCastVmObject<VmObjectPaged>(vmo)) {
      if (paged->is_slice()) {
        total++;
        if (paged->is_contiguous()) {
          contig++;
        } else if (paged->is_user_pager_backed()) {
          pager++;
        } else {
          anon++;
        }
        // Orphaned slices will not have a parent ID.
        if (paged->parent_user_id() == 0) {
          orphaned++;
          // Note that it's possible to accumulate some pages multiple times if some slices point to
          // the same parent. At the time of writing this, there were no orphaned slices found. If
          // that changes, it would make sense to deduplicate.
          size_t owner_total =
              paged->GetAttributedMemoryRangeInReferenceOwner(0, UINT64_MAX).total_bytes();
          size_t owner_visible = paged->GetAttributedMemoryInReferenceOwner().total_bytes();
          DEBUG_ASSERT(owner_total >= owner_visible);
          orphaned_bytes += owner_total - owner_visible;
        }
      }
    }
    return ZX_OK;
  });
  FormattedBytes orphaned_bytes_str(orphaned_bytes, format_unit);
  printf(
      "%zu orphaned slice vmos using %s. total slices: %zu (contig %zu, pager %zu, anon %zu)\n\n",
      orphaned, orphaned_bytes_str.c_str(), total, contig, pager, anon);
}

// Dump memory held on by VMOs that are only referenced by mappings but no VMO handles. Try to find
// pages that lie outside of any mappings, and ideally should have been reclaimed.
void DumpOrphanedMappedVmos(pretty::SizeUnit format_unit) {
  enum types : uint8_t { Root, CowClone, Slice, Reference, NumTypes };
  char type_str[][6] = {"root", "cow", "slice", "ref"};

  // Track orphaned memory vs total memory held by the VMO.
  size_t orphaned[NumTypes] = {0, 0, 0, 0};
  size_t orphaned_total = 0;
  size_t orphaned_private_bytes[NumTypes] = {0, 0, 0, 0};
  size_t orphaned_total_bytes[NumTypes] = {0, 0, 0, 0};

  // Track some additional info about the distribution of VMO types, mapping ranges, etc., that can
  // help inform any heuristics we might use to reclaim the unmapped memory.
  size_t childless[NumTypes] = {0, 0, 0, 0};
  size_t pager[NumTypes] = {0, 0, 0, 0};
  size_t non_zero_lower_bound = 0;

  VmObject::ForEachRef([&](VmObject* vmo) {
    if (auto paged = DownCastVmObject<VmObjectPaged>(vmo)) {
      // If the VMO has handles, it will have a dispatcher, which is also the child observer. So
      // check for a null child observer.
      if (paged->IsMappedByUser() && !vmo->is_child_observer_valid()) {
        // - If this is a reference or slice, then check if the parent is alive, because only then
        // could the reference be the only one keeping the parent's VmCowPages alive. (Note that we
        // do not account for sibling references of the same parent, so there is some duplication
        // here. At the time of writing this, there was no orphaned memory found in references so
        // this was not relevant.)
        // - If not a reference, then we need to make sure no child can see into us. See
        // children_cannot_access_pages() for details on how this is determined.
        bool maybe_has_orphaned_memory = paged->is_reference()
                                             ? paged->parent_user_id() == 0
                                             : paged->children_cannot_access_pages();
        if (maybe_has_orphaned_memory) {
          orphaned_total++;
          // Find the maximal range in the VMO that is mapped. There can be races and the mapping(s)
          // could go away by the time we query this range. This is fine since we're not aiming to
          // take a snapshot of the system, just an estimate.
          auto range_opt = vmo->GetMaximalMappedRange();
          if (!range_opt.has_value()) {
            return ZX_OK;
          }
          auto range = range_opt.value();
          if (range.first > 0) {
            non_zero_lower_bound++;
          }

          uint8_t type;
          switch (paged->child_type()) {
            case VmObject::kNotChild:
              type = Root;
              break;
            case VmObject::kCowClone:
              type = CowClone;
              break;
            case VmObject::kSlice:
              type = Slice;
              break;
            case VmObject::kReference:
              type = Reference;
              break;
          }

          orphaned[type]++;
          if (paged->num_children() == 0) {
            childless[type]++;
          }
          if (paged->is_user_pager_backed()) {
            pager[type]++;
          }

          // How much memory is orphaned below the lower bound of the maximal mapped range.
          auto lower_bytes = paged->is_reference()
                                 ? paged->GetAttributedMemoryRangeInReferenceOwner(0, range.first)
                                 : paged->GetAttributedMemoryInRange(0, range.first);
          // How much memory is orphaned above the upper bound of the maximal mapped range.
          auto upper_bytes =
              paged->is_reference()
                  ? paged->GetAttributedMemoryRangeInReferenceOwner(range.second, paged->size())
                  : paged->GetAttributedMemoryInRange(range.second, paged->size());
          if (type == Root) {
            DEBUG_ASSERT(lower_bytes.total_private_bytes() == lower_bytes.total_bytes());
            DEBUG_ASSERT(upper_bytes.total_private_bytes() == upper_bytes.total_bytes());
          }
          orphaned_private_bytes[type] +=
              lower_bytes.total_private_bytes() + upper_bytes.total_private_bytes();
          orphaned_total_bytes[type] += lower_bytes.total_bytes() + upper_bytes.total_bytes();
          // If we found any orphaned memory, dump some info about the VMO.
          if (lower_bytes.total_private_bytes() + upper_bytes.total_private_bytes() > 0) {
            char name[ZX_MAX_NAME_LEN];
            vmo->get_name(name, sizeof(name));
            FormattedBytes private_str(
                lower_bytes.total_private_bytes() + upper_bytes.total_private_bytes(), format_unit);
            FormattedBytes total_str(lower_bytes.total_bytes() + upper_bytes.total_bytes(),
                                     format_unit);
            FormattedBytes lower_str(lower_bytes.total_private_bytes(), format_unit);
            FormattedBytes upper_str(upper_bytes.total_private_bytes(), format_unit);
            printf(
                "orphaned %s: %s in %u mapping(s) [range: %zu, %zu],"
                " unmapped private %s (total %s) l:%s u:%s\n",
                type_str[type], name, vmo->num_mappings(), range.first, range.second,
                private_str.c_str(), total_str.c_str(), lower_str.c_str(), upper_str.c_str());
          }
        }
      }
    }
    return ZX_OK;
  });

  size_t private_bytes = 0;
  for (auto bytes : orphaned_private_bytes) {
    private_bytes += bytes;
  }
  FormattedBytes private_str(private_bytes, format_unit);
  size_t total_bytes = 0;
  for (auto bytes : orphaned_total_bytes) {
    total_bytes += bytes;
  }
  FormattedBytes total_str(total_bytes, format_unit);
  printf("%zu vmos with mappings only. unmapped private %s (total %s). non-zero lower bound: %zu\n",
         orphaned_total, private_str.c_str(), total_str.c_str(), non_zero_lower_bound);
  printf("DISTRIBUTION\n");
  for (uint8_t type = 0; type < NumTypes; type++) {
    private_str = FormattedBytes(orphaned_private_bytes[type], format_unit);
    total_str = FormattedBytes(orphaned_total_bytes[type], format_unit);
    printf("%s vmos: %zu (pager %zu, childless %zu). private %s, total %s\n", type_str[type],
           orphaned[type], pager[type], childless[type], private_str.c_str(), total_str.c_str());
  }
}

// Dumps VMOs under a VmAspace.
class AspaceVmoDumper final : public VmEnumerator {
 public:
  explicit AspaceVmoDumper(pretty::SizeUnit format_unit) : format_unit_(format_unit) {}
  zx_status_t OnVmMapping(VmMapping* map, VmAddressRegion* vmar, uint depth,
                          Guard<CriticalMutex>&) final TA_REQ(map->lock()) TA_REQ(vmar->lock()) {
    auto vmo = map->vmo_locked();
    DumpVmObject(*vmo, format_unit_, ZX_HANDLE_INVALID,
                 /* rights */ 0u,
                 /* koid */ vmo->user_id());
    return ZX_ERR_NEXT;
  }

 private:
  pretty::SizeUnit format_unit_;
};

// Dumps all VMOs associated with a process.
void DumpProcessVmObjects(zx_koid_t id, pretty::SizeUnit format_unit) {
  auto pd = ProcessDispatcher::LookupProcessById(id);
  if (!pd) {
    printf("process not found!\n");
    return;
  }

  printf("process [%" PRIu64 "]:\n", id);
  printf("Handles to VMOs:\n");
  PrintVmoDumpHeader(/* handles */ true);
  int count = 0;
  uint64_t total_size = 0;
  uint64_t total_alloc = 0;
  uint64_t total_compressed = 0;
  pd->handle_table().ForEachHandle(
      [&](zx_handle_t handle, zx_rights_t rights, const Dispatcher* disp) {
        auto vmod = DownCastDispatcher<const VmObjectDispatcher>(disp);
        if (vmod == nullptr) {
          return ZX_OK;
        }
        auto vmo = vmod->vmo();
        DumpVmObject(*vmo, format_unit, handle, rights, vmod->get_koid());

        // TODO: Doesn't handle the case where a process has multiple
        // handles to the same VMO; will double-count all of these totals.
        count++;
        total_size += vmo->size();
        // TODO: Doing this twice (here and in DumpVmObject) is a waste of
        // work, and can get out of sync.
        VmObject::AttributionCounts counts = vmo->GetAttributedMemory();
        total_alloc += counts.uncompressed_bytes;
        total_compressed += counts.compressed_bytes;
        return ZX_OK;
      });
  printf("  total: %d VMOs, size %s, alloc %s compressed %s\n", count,
         FormattedBytes(total_size, format_unit).c_str(),
         FormattedBytes(total_alloc, format_unit).c_str(),
         FormattedBytes(total_compressed, format_unit).c_str());

  // Call DumpVmObject() on all VMOs under the process's VmAspace.
  printf("Mapped VMOs:\n");
  PrintVmoDumpHeader(/* handles */ false);
  AspaceVmoDumper avd(format_unit);
  pd->EnumerateAspaceChildren(&avd);
  PrintVmoDumpHeader(/* handles */ false);
}

void DumpVmObjectCowTree(zx_koid_t id) {
  fbl::RefPtr<VmCowPages> cow_pages;
  zx_status_t status = VmObject::ForEach([id, &cow_pages = cow_pages](const VmObject& vmo) {
    if (vmo.user_id() == id) {
      if (!vmo.is_paged()) {
        printf("vmo %" PRIu64 " is not paged\n", id);
        return ZX_ERR_STOP;
      }
      const auto& paged_vmo = static_cast<const VmObjectPaged&>(vmo);
      cow_pages = paged_vmo.DebugGetCowPages();
      if (!cow_pages) {
        printf("vmo %" PRIu64 " is not fully initialized\n", id);
      }
      return ZX_ERR_STOP;
    }
    return ZX_OK;
  });
  if (status == ZX_OK) {
    printf("vmo %" PRIu64 " not found\n", id);
    return;
  }
  if (!cow_pages) {
    return;
  }
  // Walk up to the root of the tree.
  while (auto parent = cow_pages->DebugGetParent()) {
    cow_pages = parent;
  }
  cow_pages->DebugForEachDescendant([](VmCowPages* cur, uint depth) {
    AssertHeld(cur->lock_ref());
    cur->DumpLocked(depth, false);
    return ZX_OK;
  });
}

void KillProcess(zx_koid_t id) {
  // search the process list and send a kill if found
  auto pd = ProcessDispatcher::LookupProcessById(id);
  if (!pd) {
    printf("process not found!\n");
    return;
  }
  // if found, outside of the lock hit it with kill
  printf("killing process %" PRIu64 "\n", id);
  pd->Kill(ZX_TASK_RETCODE_SYSCALL_KILL);
}

// Counts memory usage under a VmAspace.
class VmCounter final : public VmEnumerator {
 public:
  zx_status_t OnVmMapping(VmMapping* map, VmAddressRegion* vmar, uint depth,
                          Guard<CriticalMutex>& guard) TA_REQ(map->lock())
      TA_REQ(vmar->lock()) override {
    const size_t map_size = map->size();
    const uint64_t object_offset = map->object_offset();
    fbl::RefPtr<VmObject> vmo = map->vmo_locked();

    guard.CallUnlocked([this, &vmo, map_size, object_offset]() {
      usage.mapped_bytes += map_size;

      const VmObject::AttributionCounts counts =
          vmo->GetAttributedMemoryInRange(object_offset, map_size);
      const uint32_t share_count = vmo->share_count();
      // Portions of the VMO itself may have sharing via copy-on-write and so, regardless of how
      // many aspaces it is mapped into (represented by share_count), it may have a mix of reported
      // private and non private bytes. At this point we can only perform approximations as we only
      // have an aggregate VMO aspace sharing factor, and aggregate counts, with no ability to
      // precisely know what portions of the private and shared vmo bytes are actually part of what
      // level of aspace sharing.
      // The approximation chosen here is to consider any shared bytes as shared, even if this
      // specific VMO does not have other mappings, and to assume that if the VMO has multiple
      // mappings that any private VMO bytes are actually shared.
      if (share_count == 1) {
        usage.private_bytes += counts.private_uncompressed_bytes;
        usage.shared_bytes += counts.uncompressed_bytes - counts.private_uncompressed_bytes;
        usage.scaled_shared_bytes +=
            (counts.scaled_uncompressed_bytes - counts.private_uncompressed_bytes) / share_count;
      } else {
        usage.shared_bytes += counts.uncompressed_bytes;
        usage.scaled_shared_bytes += counts.scaled_uncompressed_bytes / share_count;
      }
    });
    return ZX_ERR_NEXT;
  }

  VmAspace::vm_usage_t usage = {};
};

}  // namespace

zx_status_t VmAspace::GetMemoryUsage(vm_usage_t* usage) {
  VmCounter vc;
  fbl::RefPtr<VmAddressRegion> root_vmar = RootVmar();
  if (!root_vmar) {
    return ZX_ERR_INTERNAL;
  }
  if (root_vmar->EnumerateChildren(&vc) != ZX_OK) {
    *usage = {};
    return ZX_ERR_INTERNAL;
  }
  *usage = vc.usage;
  return ZX_OK;
}

namespace {

unsigned int arch_mmu_flags_to_vm_flags(arch_mmu_flags_t arch_mmu_flags) {
  if (arch_mmu_flags & ARCH_MMU_FLAG_INVALID) {
    return 0;
  }
  unsigned int ret = 0;
  if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_READ) {
    ret |= ZX_VM_PERM_READ;
  }
  if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) {
    ret |= ZX_VM_PERM_WRITE;
  }
  if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) {
    ret |= ZX_VM_PERM_EXECUTE;
  }
  return ret;
}

class AspaceEnumerator final : public VmEnumerator {
 public:
  AspaceEnumerator(VmarMapsInfoWriter& writer, size_t depth_offset, size_t max, size_t avail_start)
      : writer_(writer), depth_offset_(depth_offset), max_(max), available_(avail_start) {}
  zx_status_t OnVmMapping(VmMapping* raw_map, VmAddressRegion* vmar, uint depth,
                          Guard<CriticalMutex>& guard) TA_REQ(raw_map->lock())
      TA_REQ(vmar->lock()) override {
    fbl::RefPtr<VmMapping> map(raw_map);
    AssertHeld(map->lock_ref());
    const vaddr_t map_base = map->base();
    const size_t map_size = map->size();
    const uint64_t map_object_offset = map->object_offset();
    fbl::RefPtr<VmObject> vmo = map->vmo_locked();
    size_t region_offset = 0;
    zx_info_maps_t entry = {};

    while (region_offset < map_size && map->IsAliveLocked()) {
      const vaddr_t region_base = map_base + region_offset;
      VmMapping::FlagsRange protect_range = map->arch_mmu_flags_range_locked(region_base);
      const size_t region_len = protect_range.region_top - region_base;
      if (available_ < max_) {
        const arch_mmu_flags_t mmu_flags = protect_range.mmu_flags;
        zx_status_t status = ZX_OK;
        guard.CallUnlocked([&]() {
          vmo->get_name(entry.name, sizeof(entry.name));
          entry.base = region_base;
          entry.size = region_len;
          entry.depth = depth + depth_offset_;
          entry.type = ZX_INFO_MAPS_TYPE_MAPPING;
          zx_info_maps_mapping_t* u = &entry.u.mapping;
          u->mmu_flags = arch_mmu_flags_to_vm_flags(mmu_flags);
          u->vmo_koid = vmo->user_id();
          const uint64_t object_offset = map_object_offset + (region_base - map_base);
          u->vmo_offset = object_offset;
          const VmObject::AttributionCounts counts =
              vmo->GetAttributedMemoryInRange(object_offset, region_len);
          const vm::FractionalBytes total_scaled_bytes = counts.total_scaled_bytes();
          u->committed_bytes = counts.uncompressed_bytes;
          u->populated_bytes = counts.total_bytes();
          u->committed_private_bytes = counts.private_uncompressed_bytes;
          u->populated_private_bytes = counts.total_private_bytes();
          u->committed_scaled_bytes = counts.scaled_uncompressed_bytes.integral;
          u->populated_scaled_bytes = total_scaled_bytes.integral;
          u->committed_fractional_scaled_bytes =
              counts.scaled_uncompressed_bytes.fractional.raw_value();
          u->populated_fractional_scaled_bytes = total_scaled_bytes.fractional.raw_value();
          status = writer_.Write(entry, available_);
        });
        if (status != ZX_OK) {
          return ZX_ERR_INVALID_ARGS;
        }
      }
      available_++;
      region_offset += region_len;
    }
    return ZX_ERR_NEXT;
  }
  zx_status_t OnVmAddressRegion(VmAddressRegion* vmar, uint depth, Guard<CriticalMutex>& guard)
      TA_REQ(vmar->lock()) override {
    if (available_ < max_) {
      zx_info_maps_t entry = {};
      strlcpy(entry.name, vmar->name(), sizeof(entry.name));
      entry.base = vmar->base();
      entry.size = vmar->size();
      entry.depth = depth + depth_offset_;
      entry.type = ZX_INFO_MAPS_TYPE_VMAR;
      zx_status_t status;
      guard.CallUnlocked([&] { status = writer_.Write(entry, available_); });
      if (status != ZX_OK) {
        return ZX_ERR_INVALID_ARGS;
      }
    }
    available_++;

    return ZX_ERR_NEXT;
  }
  size_t available() const { return available_; }

 private:
  VmarMapsInfoWriter& writer_;
  const size_t depth_offset_;
  const size_t max_;
  size_t available_;
};

}  // namespace

// NOTE: Code outside of the syscall layer should not typically know about
// user_ptrs; do not use this pattern as an example.
zx_status_t GetVmAspaceMaps(VmAspace* target_aspace, VmarMapsInfoWriter& maps, size_t max,
                            size_t* actual, size_t* available) {
  DEBUG_ASSERT(target_aspace != nullptr);
  *actual = 0;
  *available = 0;

  if (target_aspace->is_destroyed()) {
    return ZX_ERR_BAD_STATE;
  }

  if (max > 0) {
    zx_info_maps_t entry = {};
    strlcpy(entry.name, target_aspace->name(), sizeof(entry.name));
    entry.base = target_aspace->base();
    entry.size = target_aspace->size();
    entry.depth = 0;
    entry.type = ZX_INFO_MAPS_TYPE_ASPACE;
    if (maps.Write(entry, 0) != ZX_OK) {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  const fbl::RefPtr<VmAddressRegion> root_vmar = target_aspace->RootVmar();
  if (!root_vmar) {
    return ZX_ERR_BAD_STATE;
  }
  AspaceEnumerator ae(maps, 1, max, 1);
  zx_status_t status = root_vmar->EnumerateChildren(&ae);
  if (status != ZX_OK) {
    return status;
  }
  *actual = ktl::min(max, ae.available());
  *available = ae.available();
  return ZX_OK;
}

// NOTE: Code outside of the syscall layer should not typically know about
// user_ptrs; do not use this pattern as an example.
zx_status_t GetVmarMaps(VmAddressRegion* target_vmar, VmarMapsInfoWriter& maps, size_t max,
                        size_t* actual, size_t* available) {
  DEBUG_ASSERT(target_vmar != nullptr);
  *actual = 0;
  *available = 0;

  AspaceEnumerator ae(maps, 0, max, 0);
  zx_status_t status = target_vmar->EnumerateChildren(&ae);
  if (status != ZX_OK) {
    return status;
  }

  *actual = ktl::min(max, ae.available());
  *available = ae.available();
  return ZX_OK;
}

namespace {

// Builds a list of all VMOs mapped into a VmAspace.
class AspaceVmoEnumerator final : public VmEnumerator {
 public:
  AspaceVmoEnumerator(VmoInfoWriter& vmos, size_t max) : vmos_(vmos), max_(max) {}
  zx_status_t OnVmMapping(VmMapping* map, VmAddressRegion*, uint, Guard<CriticalMutex>& guard)
      TA_REQ(map->lock()) override {
    if (available_ < max_) {
      fbl::RefPtr<VmObject> vmo = map->vmo_locked();
      zx_status_t status = ZX_OK;
      guard.CallUnlocked([&]() {
        zx_info_vmo_t entry =
            VmoToInfoEntry(vmo.get(), VmoOwnership::kMapping, /*handle_rights=*/0);
        status = vmos_.Write(entry, available_);
      });
      if (status != ZX_OK) {
        return status;
      }
    }
    available_++;
    return ZX_ERR_NEXT;
  }
  size_t available() const { return available_; }

 private:
  VmoInfoWriter& vmos_;
  const size_t max_;
  size_t available_ = 0;
};

}  // namespace

// NOTE: Code outside of the syscall layer should not typically know about
// user_ptrs; do not use this pattern as an example.
zx_status_t GetVmAspaceVmos(VmAspace* target_aspace, VmoInfoWriter& vmos, size_t max,
                            size_t* actual, size_t* available) {
  DEBUG_ASSERT(target_aspace != nullptr);
  DEBUG_ASSERT(actual != nullptr);
  DEBUG_ASSERT(available != nullptr);
  *actual = 0;
  *available = 0;
  if (target_aspace->is_destroyed()) {
    return ZX_ERR_BAD_STATE;
  }

  const fbl::RefPtr<VmAddressRegion> root_vmar = target_aspace->RootVmar();
  if (!root_vmar) {
    return ZX_ERR_BAD_STATE;
  }

  AspaceVmoEnumerator ave(vmos, max);
  zx_status_t status = root_vmar->EnumerateChildren(&ave);
  if (status != ZX_OK) {
    return status;
  }

  *actual = ktl::min(ave.available(), max);
  *available = ave.available();
  return ZX_OK;
}

// NOTE: Code outside of the syscall layer should not typically know about
// user_ptrs; do not use this pattern as an example.
zx_status_t GetProcessVmos(ProcessDispatcher* process, VmoInfoWriter& vmos, size_t max,
                           size_t* actual_out, size_t* available_out) {
  DEBUG_ASSERT(process != nullptr);
  DEBUG_ASSERT(actual_out != nullptr);
  DEBUG_ASSERT(available_out != nullptr);
  size_t actual = 0;
  size_t available = 0;
  // We may see multiple handles to the same VMO, but leave it to userspace to
  // do deduping.
  zx_status_t s = process->handle_table().ForEachHandleBatched(
      [&](zx_handle_t handle, zx_rights_t rights, const Dispatcher* disp) {
        auto vmod = DownCastDispatcher<const VmObjectDispatcher>(disp);
        if (vmod != nullptr) {
          available++;
          if (actual < max) {
            zx_info_vmo_t entry = VmoToInfoEntry(vmod->vmo().get(), VmoOwnership::kHandle, rights);
            if (vmos.Write(entry, actual) != ZX_OK) {
              return ZX_ERR_INVALID_ARGS;
            }
            actual++;
          }
          return ZX_OK;
        }
        auto iobd = DownCastDispatcher<const IoBufferDispatcher>(disp);
        if (iobd != nullptr) {
          available += iobd->RegionCount();
          for (size_t i = 0; i < iobd->RegionCount(); i++) {
            if (actual >= max) {
              break;
            }
            fbl::RefPtr<VmObject> vmo = iobd->GetVmo(i);
            zx_rights_t region_map_rights = iobd->GetMapRights(rights, i);
            zx_info_vmo_t entry =
                VmoToInfoEntry(vmo.get(), VmoOwnership::kIoBuffer, region_map_rights);
            if (vmos.Write(entry, actual) != ZX_OK) {
              return ZX_ERR_INVALID_ARGS;
            }
            actual++;
          }
        }
        // We can skip this if it isn't a VMO or IOB
        return ZX_OK;
      });
  if (s != ZX_OK) {
    return s;
  }
  *actual_out = actual;
  *available_out = available;
  return ZX_OK;
}

namespace {

void DumpProcessAddressSpace(zx_koid_t id) {
  auto pd = ProcessDispatcher::LookupProcessById(id);
  if (!pd) {
    printf("process %" PRIu64 " not found!\n", id);
    return;
  }

  pd->DumpAspace(true);
}

// Dumps an address space based on the arg.
void DumpAddressSpace(const cmd_args* arg) {
  if (strncmp(arg->str, "kernel", strlen(arg->str)) == 0) {
    // The arg is a prefix of "kernel".
    VmAspace::kernel_aspace()->Dump(true);
  } else {
    DumpProcessAddressSpace(arg->u);
  }
}

void DumpHandleTable() {
  printf("outstanding handles: %zu\n", Handle::diagnostics::OutstandingHandles());
  Handle::diagnostics::DumpTableInfo();
}

size_t mwd_limit_bytes = 32 * MB;
bool mwd_running;

size_t hwd_limit = 1024;
bool hwd_running;

int hwd_thread(void* arg) {
  static size_t previous_handle_count = 0u;

  for (;;) {
    auto handle_count = Handle::diagnostics::OutstandingHandles();
    if (handle_count != previous_handle_count) {
      if (handle_count > hwd_limit) {
        printf("HandleWatchdog! %zu handles outstanding (greater than limit %zu)\n", handle_count,
               hwd_limit);
      } else if (previous_handle_count > hwd_limit) {
        printf("HandleWatchdog! %zu handles outstanding (dropping below limit %zu)\n", handle_count,
               hwd_limit);
      }
    }

    previous_handle_count = handle_count;

    Thread::Current::SleepRelative(ZX_SEC(1));
  }
}

void DumpProcessMemoryUsage(const char* prefix, size_t min_bytes) {
  auto walker = MakeProcessWalker([&](ProcessDispatcher* process) {
    VmObject::AttributionCounts counts = process->GetAttributedMemory();
    if (counts.uncompressed_bytes >= min_bytes) {
      char pname[ZX_MAX_NAME_LEN];
      [[maybe_unused]] zx_status_t status = process->get_name(pname);
      DEBUG_ASSERT(status == ZX_OK);
      printf("%sproc %5" PRIu64 " %4zuM '%s'\n", prefix, process->get_koid(),
             counts.uncompressed_bytes / MB, pname);
    }
  });
  GetRootJobDispatcher()->EnumerateChildrenRecursive(&walker);
}

int mwd_thread(void* arg) {
  for (;;) {
    Thread::Current::SleepRelative(ZX_SEC(1));
    DumpProcessMemoryUsage("MemoryHog! ", mwd_limit_bytes);
  }
}

int cmd_diagnostics(int argc, const cmd_args* argv, uint32_t flags) {
  int rc = 0;

  if (argc < 2) {
    printf("not enough arguments:\n");
  usage:
    printf("%s ps                : list processes\n", argv[0].str);
    printf("%s ps help           : print header label descriptions for 'ps'\n", argv[0].str);
    printf("%s jobs              : list jobs\n", argv[0].str);
    printf("%s jobpol <koid>     : print policies for given job\n", argv[0].str);
    printf("%s mwd  <mb>         : memory watchdog\n", argv[0].str);
    printf("%s ht   <pid>        : dump process handles\n", argv[0].str);
    printf("%s hwd  <count>      : handle watchdog\n", argv[0].str);
    printf("%s vmos <pid>|all|hidden|orphaned [-u?]\n", argv[0].str);
    printf("                     : dump process/all/hidden/orphaned VMOs\n");
    printf("                 -u? : fix all sizes to the named unit\n");
    printf("                       where ? is one of [BkMGTPE]\n");
    printf("%s cow-tree <vmo>    : dump the copy-on-write tree for a vmo koid\n", argv[0].str);
    printf("%s kill <pid>        : kill process\n", argv[0].str);
    printf("%s asd  <pid>|kernel : dump process/kernel address space\n", argv[0].str);
    printf("%s htinfo            : handle table info\n", argv[0].str);
    printf("%s koid <koid>       : list all handles for a koid\n", argv[0].str);
    printf("%s koid help         : print header label descriptions for 'koid'\n", argv[0].str);
    printf("%s ch   <koid>       : dump channels for pid or for all processes,\n", argv[0].str);
    printf("                       or processes for channel koid\n");
    printf("%s sock <koid>       : dump sockets for pid or for all processes,\n", argv[0].str);
    printf("                       or processes for socket koid\n");
    printf("%s fifo <koid>       : dump fifos for pid or for all processes,\n", argv[0].str);
    printf("                       or processes for fifo koid\n");
    printf("%s eventpair <koid>  : dump event pairs for pid or for all processes,\n", argv[0].str);
    printf("                       or processes for eventpair koid\n");
    printf("%s iob <koid>        : dump io buffers for pid or for all processes,\n", argv[0].str);
    printf("                       or processes for io buffer koid\n");
    return -1;
  }

  if (strcmp(argv[1].str, "mwd") == 0) {
    if (argc == 3) {
      mwd_limit_bytes = argv[2].u * MB;
    }
    if (!mwd_running) {
      Thread* t = Thread::Create("mwd", mwd_thread, nullptr, DEFAULT_PRIORITY);
      if (t) {
        mwd_running = true;
        t->Resume();
      }
    }
  } else if (strcmp(argv[1].str, "ps") == 0) {
    if ((argc == 3) && (strcmp(argv[2].str, "help") == 0)) {
      DumpProcessListKeyMap();
    } else {
      DumpProcessList();
    }
  } else if (strcmp(argv[1].str, "jobs") == 0) {
    DumpJobList();
  } else if (strcmp(argv[1].str, "jobpol") == 0) {
    if (argc < 3)
      goto usage;
    DumpJobPolicies(argv[2].u);
  } else if (strcmp(argv[1].str, "hwd") == 0) {
    if (argc == 3) {
      hwd_limit = argv[2].u;
    }
    if (!hwd_running) {
      Thread* t = Thread::Create("hwd", hwd_thread, nullptr, DEFAULT_PRIORITY);
      if (t) {
        hwd_running = true;
        t->Resume();
      }
    }
  } else if (strcmp(argv[1].str, "ht") == 0) {
    if (argc < 3)
      goto usage;
    DumpProcessHandles(argv[2].u);
  } else if (strcmp(argv[1].str, "ch") == 0) {
    if (argc == 3) {
      DumpPeerDispatchersByKoid(ZX_OBJ_TYPE_CHANNEL, argv[2].u);
    } else {
      DumpAllPeerDispatchers(ZX_OBJ_TYPE_CHANNEL);
    }
  } else if (strcmp(argv[1].str, "sock") == 0) {
    if (argc == 3) {
      DumpPeerDispatchersByKoid(ZX_OBJ_TYPE_SOCKET, argv[2].u);
    } else {
      DumpAllPeerDispatchers(ZX_OBJ_TYPE_SOCKET);
    }
  } else if (strcmp(argv[1].str, "fifo") == 0) {
    if (argc == 3) {
      DumpPeerDispatchersByKoid(ZX_OBJ_TYPE_FIFO, argv[2].u);
    } else {
      DumpAllPeerDispatchers(ZX_OBJ_TYPE_FIFO);
    }
  } else if (strcmp(argv[1].str, "eventpair") == 0) {
    if (argc == 3) {
      DumpPeerDispatchersByKoid(ZX_OBJ_TYPE_EVENTPAIR, argv[2].u);
    } else {
      DumpAllPeerDispatchers(ZX_OBJ_TYPE_EVENTPAIR);
    }
  } else if (strcmp(argv[1].str, "iob") == 0) {
    if (argc == 3) {
      DumpPeerDispatchersByKoid(ZX_OBJ_TYPE_IOB, argv[2].u);
    } else {
      DumpAllPeerDispatchers(ZX_OBJ_TYPE_IOB);
    }
  } else if (strcmp(argv[1].str, "vmos") == 0) {
    if (argc < 3)
      goto usage;
    pretty::SizeUnit format_unit = pretty::SizeUnit::kAuto;
    if (argc >= 4) {
      if (!strncmp(argv[3].str, "-u", sizeof("-u") - 1)) {
        format_unit = static_cast<pretty::SizeUnit>(argv[3].str[sizeof("-u") - 1]);
      } else {
        printf("dunno '%s'\n", argv[3].str);
        goto usage;
      }
    }
    if (strcmp(argv[2].str, "all") == 0) {
      DumpAllVmObjects(/*hidden_only=*/false, format_unit);
    } else if (strcmp(argv[2].str, "hidden") == 0) {
      DumpAllVmObjects(/*hidden_only=*/true, format_unit);
    } else if (strcmp(argv[2].str, "orphaned") == 0) {
      DumpOrphanedSlices(format_unit);
      DumpOrphanedMappedVmos(format_unit);
    } else {
      DumpProcessVmObjects(argv[2].u, format_unit);
    }
  } else if (strcmp(argv[1].str, "cow-tree") == 0) {
    if (argc < 3)
      goto usage;
    DumpVmObjectCowTree(argv[2].u);
  } else if (strcmp(argv[1].str, "kill") == 0) {
    if (argc < 3)
      goto usage;
    KillProcess(argv[2].u);
  } else if (strcmp(argv[1].str, "asd") == 0) {
    if (argc < 3)
      goto usage;
    DumpAddressSpace(&argv[2]);
  } else if (strcmp(argv[1].str, "htinfo") == 0) {
    if (argc != 2)
      goto usage;
    DumpHandleTable();
  } else if (strcmp(argv[1].str, "koid") == 0) {
    if (argc < 3)
      goto usage;

    if (strcmp(argv[2].str, "help") == 0) {
      DumpHandleRightsKeyMap();
    } else {
      DumpHandlesForKoid(argv[2].u);
    }
  } else {
    printf("unrecognized subcommand '%s'\n", argv[1].str);
    goto usage;
  }
  return rc;
}

}  // namespace

STATIC_COMMAND_START
STATIC_COMMAND("zx", "kernel object diagnostics", &cmd_diagnostics)
STATIC_COMMAND_END(zx)
