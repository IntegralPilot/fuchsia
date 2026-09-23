// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <lib/counters.h>
#include <lib/crashlog.h>
#include <lib/elfldltl/machine.h>
#include <lib/instrumentation/vmo.h>
#include <lib/page/size.h>
#include <lib/userabi/userboot.h>
#include <lib/userabi/vdso.h>
#include <lib/zircon-internal/default_stack_size.h>
#include <platform.h>
#include <stdio.h>
#include <trace.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/vector.h>
#include <ktl/bit.h>
#include <ktl/concepts.h>
#include <ktl/ranges.h>
#include <ktl/source_location.h>
#include <ktl/utility.h>
#include <lk/init.h>
#include <object/channel_dispatcher.h>
#include <object/handle.h>
#include <object/job_dispatcher.h>
#include <object/log_dispatcher.h>
#include <object/message_packet.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>
#include <object/vm_address_region_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <phys/handoff.h>
#include <platform/crashlog.h>
#include <vm/vm_object_paged.h>

#if ENABLE_ENTROPY_COLLECTOR_TEST
#include <lib/crypto/entropy/quality_test.h>
#endif

#ifdef __aarch64__
#include <arch/arm64/feature.h>
#endif

#include "elf.h"

#include <ktl/enforce.h>

#define RETURN_IF_NOT_OK(expr, ...) \
  do {                              \
    zx_status_t eval_expr = (expr); \
    if (eval_expr != ZX_OK) {       \
      KernelOops(#expr, eval_expr); \
      return __VA_ARGS__;           \
    }                               \
  } while (0)

#define RETURN_IF_NOT(predicate, ...) \
  do {                                \
    if (!(predicate)) {               \
      KernelOops(#predicate, false);  \
      return __VA_ARGS__;             \
    }                                 \
  } while (0)

namespace {

template <typename T>
void KernelOops(const char* expression, T actual,
                ktl::source_location where = ktl::source_location::current()) {
  KERNEL_OOPS("[%s:%u] Expectation failure (%s was %d).\n", where.file_name(), where.line(),
              expression, actual);
}

class VmoBuffer {
 public:
  explicit VmoBuffer(fbl::RefPtr<VmObjectPaged> vmo) : size_(vmo->size()), vmo_(ktl::move(vmo)) {}

  explicit VmoBuffer(size_t size = kPageSize, uint32_t options = VmObjectPaged::kResizable) {
    zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, options, size, &vmo_);
    ZX_ASSERT(status == ZX_OK);
    size_ = vmo_->size();
  }

  int Write(const ktl::string_view str) {
    // Enlarge the VMO as needed if it was created as resizable.
    if (str.size() > size_ - offset_ && vmo_->is_resizable()) {
      DEBUG_ASSERT(vmo_->size() < offset_ + str.size());
      size_t minimum_size = offset_ + str.size();
      size_t page_aligned_size = fbl::round_up(minimum_size, size_t{kPageSize});
      zx_status_t status = vmo_->Resize(page_aligned_size);
      if (status == ZX_OK) {
        // Update unlocked cache of the size.
        size_ = vmo_->size();
      } else if (offset_ == size_) {
        // None left to write without the resize.
        // Otherwise proceed to write what can be written.
        return static_cast<int>(status);
      }
    }

    const size_t todo = ktl::min(str.size(), size_ - offset_);

    if (zx_status_t res = vmo_->Write(str.data(), offset_, todo); res != ZX_OK) {
      DEBUG_ASSERT(static_cast<int>(res) < 0);
      return res;
    }

    offset_ += todo;
    DEBUG_ASSERT(todo <= ktl::numeric_limits<int>::max());
    return static_cast<int>(todo);
  }

  const fbl::RefPtr<VmObjectPaged>& vmo() const { return vmo_; }

  size_t stream_size() const { return offset_; }

 private:
  size_t offset_{0};
  size_t size_;
  fbl::RefPtr<VmObjectPaged> vmo_;
};

constexpr const char kStackVmoName[] = "userboot-initial-stack";
constexpr const char kCrashlogVmoName[] = "crashlog";
constexpr const char kBootOptionsVmoname[] = "boot-options.txt";

KCOUNTER(timeline_userboot, "boot.timeline.userboot")
KCOUNTER(init_time, "init.userboot.time.msec")

class Userboot {
 public:
  Userboot(Userboot&&) = default;

  Userboot(HandoffEnd::Elf userboot, HandoffEnd::Elf vdso)
      : userboot_elf_{ktl::move(userboot)}, vdso_elf_{ktl::move(vdso)} {}

  [[nodiscard]] zx_status_t Map(VmAddressRegionDispatcher& root_vmar, HandleOwner& out_vmar) {
    // Map in the userboot image along with the vDSO.
    zx::result mapped = MapElfAndVdso(root_vmar);
    RETURN_IF_NOT_OK(mapped.status_value(), mapped.status_value());
    out_vmar = ktl::move(mapped->userboot_vmar);
    dprintf(SPEW, "userboot: %-31s @  %#" PRIxPTR "\n", "entry point", mapped->userboot_entry);

    // Set up the stack.
    zx::result<uintptr_t> sp = MapStack(root_vmar, mapped->stack_size);
    RETURN_IF_NOT_OK(sp.status_value(), sp.status_value());

    entry_ = mapped->userboot_entry;
    sp_ = sp.value();
    vdso_base_ = mapped->vdso_base;
    return ZX_OK;
  }

  [[nodiscard]] zx_status_t Start(ProcessDispatcher& process, fbl::RefPtr<ThreadDispatcher> thread,
                                  HandleOwner arg_handle) const {
    // Start the process running.
    return process.Start(ktl::move(thread), entry_, sp_, ktl::move(arg_handle), vdso_base_);
  }

 private:
  struct Mapped {
    HandleOwner userboot_vmar;
    zx_vaddr_t userboot_entry;
    zx_vaddr_t vdso_base;
    size_t stack_size;
  };

  zx::result<Mapped> MapElfAndVdso(VmAddressRegionDispatcher& root_vmar) {
    // Map userboot proper.
    zx::result userboot = MapHandoffElf(ktl::move(userboot_elf_), root_vmar);
    if (userboot.is_error()) {
      return userboot.take_error();
    }

    // Map the vDSO.
    zx::result vdso = MapHandoffElf(ktl::move(vdso_elf_), root_vmar);
    if (vdso.is_error()) {
      return vdso.take_error();
    }

    RETURN_IF_NOT(userboot->stack_size, zx::error(ZX_ERR_INVALID_ARGS));
    return zx::ok(Mapped{
        .userboot_vmar = ktl::move(userboot->vmar),
        .userboot_entry = userboot->entry,
        .vdso_base = vdso->vaddr_start,
        .stack_size = *userboot->stack_size,
    });
  }

  // Map the stack anywhere, in its own VMAR and a one-page guard region below.
  static zx::result<uintptr_t> MapStack(VmAddressRegionDispatcher& root_vmar, size_t stack_size) {
    fbl::RefPtr<VmObjectPaged> stack_vmo;
    zx_status_t status;

    RETURN_IF_NOT_OK(status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY | PMM_ALLOC_FLAG_CAN_WAIT,
                                                    0u, stack_size, &stack_vmo),
                     zx::error(status));
    stack_vmo->set_name(kStackVmoName, sizeof(kStackVmoName) - 1);

    const size_t vmar_size = stack_size + kPageSize;
    KernelHandle<VmAddressRegionDispatcher> vmar_handle;
    zx_rights_t vmar_rights;
    RETURN_IF_NOT_OK(
        status = root_vmar.Allocate(
            0, vmar_size, ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC,
            &vmar_handle, &vmar_rights),
        zx::error(status));

    zx::result<VmAddressRegion::MapResult> map_result = vmar_handle.dispatcher()->Map(
        kPageSize, stack_vmo, 0, stack_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC);
    RETURN_IF_NOT_OK(map_result.status_value(), map_result.take_error());
    const uintptr_t stack_base = map_result->base;
    const uintptr_t sp = elfldltl::AbiTraits<>::InitialStackPointer(stack_base, stack_size);
    dprintf(SPEW, "userboot: %-31s @ [%#" PRIxPTR ", %#" PRIxPTR ")\n", "stack mapped", stack_base,
            stack_base + stack_size);
    constexpr auto hex_width = [](auto x) { return 2 + ((ktl::bit_width(x) + 3) / 4); };
    dprintf(SPEW, "userboot: %-31s @ %#*" PRIxPTR "\n", "sp",
            hex_width(stack_base) + 3 + hex_width(sp), sp);

    zx_rights_t vmo_rights;
    KernelHandle<VmObjectDispatcher> vmo_handle;
    RETURN_IF_NOT_OK(status = VmObjectDispatcher::Create(
                         ktl::move(stack_vmo), stack_size,
                         VmObjectDispatcher::InitialMutability::kMutable, &vmo_handle, &vmo_rights),
                     zx::error(status));

    return zx::ok(sp);
  }

  HandoffEnd::Elf userboot_elf_;
  HandoffEnd::Elf vdso_elf_;
  zx_vaddr_t entry_{0};
  uintptr_t sp_{0};
  zx_vaddr_t vdso_base_{0};
};

// Keep a global reference to the kcounters vmo so that the kcounters
// memory always remains valid, even if userspace closes the last handle.
fbl::RefPtr<VmObject> kcounters_vmo_ref;

// Get a handle to a VM object, with full rights except perhaps for writing.
zx_status_t get_vmo_handle(fbl::RefPtr<VmObject> vmo, bool readonly, uint64_t stream_size,
                           HandleOwner& out_handle,
                           fbl::RefPtr<VmObjectDispatcher>* disp_ptr = nullptr) {
  if (!vmo)
    return ZX_ERR_NO_MEMORY;

  zx_rights_t rights;
  KernelHandle<VmObjectDispatcher> vmo_kernel_handle;
  zx_status_t result = VmObjectDispatcher::Create(ktl::move(vmo), stream_size,
                                                  VmObjectDispatcher::InitialMutability::kMutable,
                                                  &vmo_kernel_handle, &rights);
  if (result == ZX_OK) {
    if (disp_ptr)
      *disp_ptr = vmo_kernel_handle.dispatcher();
    if (readonly)
      rights &= ~ZX_RIGHT_WRITE;
    out_handle = Handle::Make(ktl::move(vmo_kernel_handle), rights);
  }
  return result;
}

HandleOwner get_job_handle() {
  return Handle::Dup(GetRootJobHandle(), JobDispatcher::default_rights());
}

// Converts platform crashlog into a VMO
zx_status_t crashlog_to_vmo(fbl::RefPtr<VmObject>* out, size_t* out_size) {
  PlatformCrashlog::Interface& crashlog = PlatformCrashlog::Get();

  size_t size = crashlog.Recover(nullptr);
  fbl::RefPtr<VmObjectPaged> crashlog_vmo;
  size_t aligned_size;
  zx_status_t status;
  RETURN_IF_NOT_OK(status = VmObject::RoundSize(size, &aligned_size), status);
  RETURN_IF_NOT_OK(
      status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, aligned_size, &crashlog_vmo), status);

  if (size) {
    VmoBuffer vmo_buffer{crashlog_vmo};
    FILE vmo_file{&vmo_buffer};
    crashlog.Recover(&vmo_file);
  }

  crashlog_vmo->set_name(kCrashlogVmoName, sizeof(kCrashlogVmoName) - 1);

  // Stash the recovered crashlog so that it may be propagated to the next
  // kernel instance in case we later mexec.
  crashlog_stash(crashlog_vmo);

  *out = ktl::move(crashlog_vmo);
  *out_size = size;

  // Now that we have recovered the old crashlog, enable crashlog uptime
  // updates.  This will cause systems with a RAM based crashlog to periodically
  // create a payload-less crashlog indicating a SW reboot reason of "unknown"
  // along with an uptime indicator.  If the system spontaneously reboots (due
  // to something like a WDT, or brownout) we will be able to recover this log
  // and know that we spontaneously rebooted, and have some idea of how long we
  // were running before we did.
  crashlog.EnableCrashlogUptimeUpdates(true);
  return ZX_OK;
}

zx_status_t bootstrap_vmos(HandoffEnd handoff_end, fbl::Vector<HandleOwner>& handles,
                           ktl::optional<Userboot>& out_userboot) {
  fbl::AllocChecker ac;
  auto push_handle = [&handles, &ac](HandleOwner handle) {
    if (handle) {
      handles.push_back(ktl::move(handle), &ac);
      ZX_ASSERT(ac.check());
    }
  };

  // ZBI VMO
  push_handle(ktl::move(handoff_end.zbi));

  // vDSO VMOs & TimeValues
  KernelHandle<VmObjectDispatcher> vdso_kernel_handles[VDso::kNumVdsoVariants];
  KernelHandle<VmObjectDispatcher> time_values_handle;
  const VDso* vdso =
      VDso::Create(handoff_end.vdso, ktl::span{vdso_kernel_handles}, &time_values_handle);

  HandleOwner time_values =
      Handle::Make(ktl::move(time_values_handle), (vdso->vmo_rights() & (~ZX_RIGHT_EXECUTE)));
  RETURN_IF_NOT(time_values, ZX_ERR_NO_MEMORY);
  push_handle(ktl::move(time_values));

  if (BootOptions::Get()->always_use_next_vdso) {
    ktl::swap(vdso_kernel_handles[0], vdso_kernel_handles[1]);
  }
  for (size_t i = 0; i < VDso::kNumVdsoVariants; ++i) {
    RETURN_IF_NOT(vdso_kernel_handles[i].dispatcher(), ZX_ERR_NO_MEMORY);
    HandleOwner vdso_h = Handle::Make(ktl::move(vdso_kernel_handles[i]), vdso->vmo_rights());
    RETURN_IF_NOT(vdso_h, ZX_ERR_NO_MEMORY);
    push_handle(ktl::move(vdso_h));
  }

  // Crashlog
  fbl::RefPtr<VmObject> crashlog_vmo;
  size_t crashlog_size = 0;
  RETURN_IF_NOT_OK(crashlog_to_vmo(&crashlog_vmo, &crashlog_size), ZX_ERR_NO_MEMORY);
  HandleOwner crashlog_handle;
  RETURN_IF_NOT_OK(get_vmo_handle(crashlog_vmo, true, crashlog_size, crashlog_handle),
                   ZX_ERR_NO_MEMORY);
  push_handle(ktl::move(crashlog_handle));

  // Boot options
  {
    VmoBuffer boot_options;
    FILE boot_options_file{&boot_options};
    BootOptions::Get()->Show(/*defaults=*/false, &boot_options_file);
    boot_options.vmo()->set_name(kBootOptionsVmoname, sizeof(kBootOptionsVmoname) - 1);
    HandleOwner boot_options_handle;
    RETURN_IF_NOT_OK(
        get_vmo_handle(boot_options.vmo(), false, boot_options.stream_size(), boot_options_handle),
        ZX_ERR_NO_MEMORY);
    push_handle(ktl::move(boot_options_handle));
  }

#if ENABLE_ENTROPY_COLLECTOR_TEST
  RETURN_IF_NOT(!crypto::entropy::entropy_was_lost, ZX_ERR_NO_MEMORY);
  HandleOwner entropy_handle;
  RETURN_IF_NOT_OK(get_vmo_handle(crypto::entropy::entropy_vmo, true,
                                  crypto::entropy::entropy_vmo_stream_size, entropy_handle),
                   ZX_ERR_NO_MEMORY);
  push_handle(ktl::move(entropy_handle));
#endif

  // kcounters names table
  fbl::RefPtr<VmObjectPaged> kcountdesc_vmo;
  RETURN_IF_NOT_OK(VmObjectPaged::CreateFromWiredPages(
                       CounterDesc().VmoData(), CounterDesc().VmoDataSize(), true, &kcountdesc_vmo),
                   ZX_ERR_NO_MEMORY);
  kcountdesc_vmo->set_name(counters::DescriptorVmo::kVmoName,
                           sizeof(counters::DescriptorVmo::kVmoName) - 1);
  HandleOwner kcountdesc_handle;
  RETURN_IF_NOT_OK(get_vmo_handle(ktl::move(kcountdesc_vmo), true, CounterDesc().VmoStreamSize(),
                                  kcountdesc_handle),
                   ZX_ERR_NO_MEMORY);
  push_handle(ktl::move(kcountdesc_handle));

  // kcounters live data
  fbl::RefPtr<VmObjectPaged> kcounters_vmo;
  RETURN_IF_NOT_OK(
      VmObjectPaged::CreateFromWiredPages(CounterArena().VmoData(), CounterArena().VmoDataSize(),
                                          false, &kcounters_vmo),
      ZX_ERR_NO_MEMORY);
  kcounters_vmo_ref = kcounters_vmo;
  kcounters_vmo->set_name(counters::kArenaVmoName, sizeof(counters::kArenaVmoName) - 1);
  HandleOwner kcounters_handle;
  RETURN_IF_NOT_OK(get_vmo_handle(ktl::move(kcounters_vmo), true, CounterArena().VmoStreamSize(),
                                  kcounters_handle),
                   ZX_ERR_NO_MEMORY);
  push_handle(ktl::move(kcounters_handle));

  // midr.txt
  {
    constexpr ktl::string_view kMidrTxt = "midr.txt";
    VmoBuffer midr_txt;
    FILE midr_txt_file{&midr_txt};
#if defined(__aarch64__) && ZX_DEBUG_ASSERT_IMPLEMENTED
    arm64_print_midr_cpu_name(&midr_txt_file);
#endif
    if (midr_txt.stream_size() > 0) {
      midr_txt.vmo()->set_name(kMidrTxt.data(), kMidrTxt.size());
    }
    HandleOwner midr_handle;
    RETURN_IF_NOT_OK(get_vmo_handle(midr_txt.vmo(), false, midr_txt.stream_size(), midr_handle),
                     ZX_ERR_NO_MEMORY);
    push_handle(ktl::move(midr_handle));
  }

  // Instrumentation VMOs
  Handle* inst_handles[InstrumentationData::vmo_count()] = {};
  RETURN_IF_NOT_OK(InstrumentationData::GetVmos(inst_handles), ZX_ERR_NO_MEMORY);
  for (Handle* h : inst_handles) {
    if (h) {
      push_handle(HandleOwner(h));
    }
  }

  // Extra phys VMOs
  for (auto& extra_vmo : handoff_end.extra_phys_vmos) {
    if (extra_vmo) {
      push_handle(ktl::move(extra_vmo));
    }
  }

  out_userboot.emplace(ktl::move(handoff_end.userboot), ktl::move(handoff_end.vdso));
  return ZX_OK;
}

class BootstrapChannel {
 public:
  static zx::result<> Create(ProcessDispatcher& process,
                             ktl::optional<BootstrapChannel>& out_channel) {
    // Make the channel that will hold the message.
    KernelHandle<ChannelDispatcher> user_handle, kernel_handle;
    zx_rights_t channel_rights;
    zx_status_t res;
    RETURN_IF_NOT_OK(res = ChannelDispatcher::Create(&user_handle, &kernel_handle, &channel_rights),
                     zx::make_result(res));
    out_channel.emplace();
    out_channel->user_handle_ = Handle::Make(ktl::move(user_handle), channel_rights);
    out_channel->send_ = kernel_handle.release();
    return zx::ok();
  }

  // Send a message containing only handles.
  template <ktl::ranges::sized_range R>
    requires(ktl::same_as<ktl::ranges::range_reference_t<R>, HandleOwner&>)
  zx_status_t SendHandles(R&& handles) {
    RETURN_IF_NOT(send_, ZX_ERR_BAD_STATE);
    const uint32_t count = static_cast<uint32_t>(ktl::ranges::size(handles));
    MessagePacketPtr msg;
    zx_status_t status = MessagePacket::Create(nullptr, 0, count, &msg);
    RETURN_IF_NOT_OK(status, status);
    msg->set_owns_handles(true);
    ktl::span msg_handles{msg->mutable_handles(), msg->num_handles()};
    ZX_DEBUG_ASSERT(msg_handles.size() == count);
    auto it = msg_handles.begin();
    for (HandleOwner& handle : handles) {
      RETURN_IF_NOT(handle, ZX_ERR_BAD_HANDLE);
      *it++ = handle.release();
    }
    return send_->Write(ZX_KOID_INVALID, ktl::move(msg));
  }

  HandleOwner TakeUserHandle() { return ktl::move(user_handle_); }

 private:
  HandleOwner user_handle_;
  fbl::RefPtr<ChannelDispatcher> send_;
};

void MakeThread(fbl::RefPtr<ProcessDispatcher> process,
                fbl::RefPtr<ThreadDispatcher>& out_dispatcher, HandleOwner& out_handle) {
  ASSERT(out_dispatcher == nullptr);
  KernelHandle<ThreadDispatcher> thread_handle;
  zx_rights_t thread_rights;
  RETURN_IF_NOT_OK(
      ThreadDispatcher::Create(ktl::move(process), 0, "userboot", &thread_handle, &thread_rights));
  RETURN_IF_NOT_OK(thread_handle.dispatcher()->Initialize());
  out_dispatcher = thread_handle.dispatcher();
  out_handle = Handle::Make(ktl::move(thread_handle), thread_rights);
}

}  // namespace

void userboot_init(HandoffEnd handoff_end) {
  // Create process.
  KernelHandle<ProcessDispatcher> process_handle;
  KernelHandle<VmAddressRegionDispatcher> vmar_handle;
  zx_rights_t process_rights, vmar_rights;
  zx_status_t status =
      ProcessDispatcher::Create(GetRootJobDispatcher(), "userboot", 0, &process_handle,
                                &process_rights, &vmar_handle, &vmar_rights);
  ASSERT(status == ZX_OK);

  // Create a root job observer, restarting the system if the root job becomes
  // childless. From now, the life of the system is bound to this first process.
  StartRootJobObserver();
  fbl::RefPtr<ProcessDispatcher> process = process_handle.dispatcher();
  auto kill_userboot =
      fit::defer([process]() { process->Kill(ZX_TASK_RETCODE_CRITICAL_PROCESS_KILL); });

  // Create thread.
  fbl::RefPtr<ThreadDispatcher> thread;
  HandleOwner thread_self;
  MakeThread(process_handle.dispatcher(), thread, thread_self);
  RETURN_IF_NOT(thread);

  // Create bootstrap channel.
  ktl::optional<BootstrapChannel> bootstrap_channel;
  RETURN_IF_NOT_OK(
      BootstrapChannel::Create(*process_handle.dispatcher(), bootstrap_channel).status_value());
  RETURN_IF_NOT(bootstrap_channel.has_value());

  // Handles for the system capability message.
  fbl::Vector<HandleOwner> system_capability_handles;

  // Pack up VMOs and create userboot loader object.
  ktl::optional<Userboot> userboot;
  RETURN_IF_NOT_OK(bootstrap_vmos(ktl::move(handoff_end), system_capability_handles, userboot));
  RETURN_IF_NOT(userboot.has_value());

  // Map userboot and obtain vmar_loaded.
  HandleOwner vmar_loaded;
  RETURN_IF_NOT_OK(userboot->Map(*vmar_handle.dispatcher(), vmar_loaded));
  RETURN_IF_NOT(vmar_loaded);

  // Convert process and root VMAR handles.
  HandleOwner proc_self = Handle::Make(ktl::move(process_handle), process_rights);
  HandleOwner vmar_root_self = Handle::Make(ktl::move(vmar_handle), vmar_rights);
  RETURN_IF_NOT(proc_self);
  RETURN_IF_NOT(vmar_root_self);

  // Create log dispatcher.
  KernelHandle<LogDispatcher> log;
  zx_rights_t log_rights;
  RETURN_IF_NOT_OK(LogDispatcher::Create(0, &log, &log_rights));

  HandleOwner log_for_system_capability = Handle::Make(log.dispatcher(), log_rights);
  HandleOwner log_for_process_capability = Handle::Make(ktl::move(log), log_rights);

  // Send message 1: the process capability message.
  // This contains essential handles describing the userboot process itself.
  ktl::array<HandleOwner, 5> process_capability_handles = {
      ktl::move(log_for_process_capability),
      Handle::Dup(*proc_self, proc_self->rights()),
      Handle::Dup(*vmar_root_self, vmar_root_self->rights()),
      Handle::Dup(*thread_self, thread_self->rights()),
      Handle::Dup(*vmar_loaded, vmar_loaded->rights()),
  };
  RETURN_IF_NOT_OK(bootstrap_channel->SendHandles(process_capability_handles));

  fbl::AllocChecker ac;
  auto push_handle = [&system_capability_handles, &ac](HandleOwner handle) {
    if (handle) {
      system_capability_handles.push_back(ktl::move(handle), &ac);
      ZX_ASSERT(ac.check());
    }
  };

  // Add process, resource, job, and log handles for the system capability message.
  push_handle(ktl::move(log_for_system_capability));
  push_handle(ktl::move(proc_self));
  push_handle(ktl::move(vmar_root_self));
  push_handle(ktl::move(thread_self));
  push_handle(ktl::move(vmar_loaded));
  push_handle(get_job_handle());
  push_handle(get_resource_handle(ZX_RSRC_KIND_MMIO));
  push_handle(get_resource_handle(ZX_RSRC_KIND_IRQ));
#if defined(__x86_64__)
  push_handle(get_resource_handle(ZX_RSRC_KIND_IOPORT));
#elif defined(__aarch64__)
  push_handle(get_resource_handle(ZX_RSRC_KIND_SMC));
#endif
  push_handle(get_resource_handle(ZX_RSRC_KIND_SYSTEM));

  // Send message 2: the system capability message.
  if (zx_status_t send_status = bootstrap_channel->SendHandles(system_capability_handles);
      send_status != ZX_OK) {
    zx_info_process_t info = process->GetInfo();
    KERNEL_OOPS("write on userboot bootstrap channel failed: %d; process retcode %" PRId64
                ", flags %#" PRIx32 "\n",
                send_status, info.return_code, info.flags);
    return;
  }

  // Start userboot process now that all bootstrap messages are sent.
  RETURN_IF_NOT_OK(
      userboot->Start(*process, ktl::move(thread), bootstrap_channel->TakeUserHandle()));

  kill_userboot.cancel();

  timeline_userboot.Set(current_mono_ticks());
  init_time.Add(current_mono_time() / 1000000LL);
}
