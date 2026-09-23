// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/heap.h>
#include <lib/stall.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zircon-internal/macros.h>
#include <lib/zx/result.h>
#include <platform.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/iob.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/resource.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstdint>

#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <kernel/mp.h>
#include <kernel/scheduler.h>
#include <kernel/stats.h>
#include <ktl/algorithm.h>
#include <ktl/iterator.h>
#include <object/bus_transaction_initiator_dispatcher.h>
#include <object/clock_dispatcher.h>
#include <object/diagnostics.h>
#include <object/handle.h>
#include <object/interrupt_dispatcher.h>
#include <object/io_buffer_dispatcher.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/resource.h>
#include <object/resource_dispatcher.h>
#include <object/stream_dispatcher.h>
#include <object/thread_dispatcher.h>
#include <object/vcpu_dispatcher.h>
#include <object/vm_address_region_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <vm/compression.h>
#include <vm/discardable_vmo_tracker.h>
#include <vm/memory_stats.h>
#include <vm/pmm.h>
#include <vm/vm.h>

#include "object_info_priv.h"

#include <ktl/enforce.h>

namespace {

// Specialize the VmoInfoWriter to work for any T that is a subset of zx_info_vmo_t. This is
// currently true for v1 and v2 (v2 being the current version). Being a subset the full
// zx_info_vmo_t can just be casted and copied.
template <typename T>
class SubsetVmoInfoWriter : public VmoInfoWriter {
 public:
  explicit SubsetVmoInfoWriter(user_out_ptr<T> out) : out_(out) {}
  ~SubsetVmoInfoWriter() override = default;
  zx_status_t Write(const zx_info_vmo_t& vmo, size_t offset) override {
    T versioned_vmo = VmoInfoToVersion<T>(vmo);
    return out_.element_offset(offset + base_offset_).copy_to_user(versioned_vmo);
  }
  UserCopyCaptureFaultsResult WriteCaptureFaults(const zx_info_vmo_t& vmo, size_t offset) override {
    T versioned_vmo = VmoInfoToVersion<T>(vmo);
    return out_.element_offset(offset + base_offset_).copy_to_user_capture_faults(versioned_vmo);
  }
  void AddOffset(size_t offset) override { base_offset_ += offset; }

 private:
  static_assert(sizeof(T) <= sizeof(zx_info_vmo_t));
  user_out_ptr<T> out_;
  size_t base_offset_ = 0;
};

template <typename T>
class SubsetVmarMapsInfoWriter : public VmarMapsInfoWriter {
 public:
  explicit SubsetVmarMapsInfoWriter(user_out_ptr<T> out) : out_(out) {}
  ~SubsetVmarMapsInfoWriter() override = default;
  zx_status_t Write(const zx_info_maps_t& maps, size_t offset) override {
    T versioned_maps = MapsInfoToVersion<T>(maps);
    return out_.element_offset(offset + base_offset_).copy_to_user(versioned_maps);
  }
  UserCopyCaptureFaultsResult WriteCaptureFaults(const zx_info_maps_t& maps,
                                                 size_t offset) override {
    T versioned_maps = MapsInfoToVersion<T>(maps);
    return out_.element_offset(offset + base_offset_).copy_to_user_capture_faults(versioned_maps);
  }
  void AddOffset(size_t offset) override { base_offset_ += offset; }

 private:
  static_assert(sizeof(T) <= sizeof(zx_info_maps_t));
  user_out_ptr<T> out_;
  size_t base_offset_ = 0;
};

// TODO: figure out a better handle to hang this off to and push this copy code into
// that dispatcher.
zx_info_cpu_stats_t GetCPUStats(uint32_t cpu_num) {
  const cpu_stats cpu_stats = Scheduler::GetProjectedCpuStats(cpu_num);
  zx_info_cpu_stats_t stats = {};

  stats.cpu_number = cpu_num;
  stats.flags = mp_is_cpu_online(cpu_num) ? ZX_INFO_CPU_STATS_FLAG_ONLINE : 0;
  stats.idle_time = cpu_stats.idle_time;
  stats.normalized_busy_time = cpu_stats.normalized_busy_time;
  stats.reschedules = cpu_stats.reschedules;
  stats.context_switches = cpu_stats.context_switches;
  stats.irq_preempts = 0;  // deprecated, not tracking different types of preemptions.
  stats.preempts = cpu_stats.preempts;
  stats.yields = cpu_stats.yields;
  stats.ints = cpu_stats.interrupts;
  stats.timer_ints = cpu_stats.timer_ints;
  stats.timers = cpu_stats.timers;
  stats.page_faults = cpu_stats.page_faults;
  stats.exceptions = 0;  // deprecated, use "kcounter" command for now.
  stats.syscalls = cpu_stats.syscalls;
  stats.reschedule_ipis = cpu_stats.reschedule_ipis;
  stats.generic_ipis = cpu_stats.generic_ipis;
  stats.active_energy_consumption_nj = cpu_stats.active_energy_consumption_nj;
  stats.idle_energy_consumption_nj = cpu_stats.idle_energy_consumption_nj;

  return stats;
}

zx_info_guest_stats_t GetGuestCPUStats(uint32_t cpu_num) {
  const auto* cpu = &percpu::Get(cpu_num);
  zx_info_guest_stats_t stats = {};
  stats.cpu_number = cpu_num;
  stats.flags = mp_is_cpu_online(cpu_num) ? ZX_INFO_CPU_STATS_FLAG_ONLINE : 0;

  stats.vm_entries = cpu->gstats.vm_entries;
  stats.vm_exits = cpu->gstats.vm_exits;
#ifdef __aarch64__
  stats.wfi_wfe_instructions = cpu->gstats.wfi_wfe_instructions;
  stats.system_instructions = cpu->gstats.system_instructions;
  stats.instruction_aborts = cpu->gstats.instruction_aborts;
  stats.data_aborts = cpu->gstats.data_aborts;
  stats.smc_instructions = cpu->gstats.smc_instructions;
  stats.interrupts = cpu->gstats.interrupts;
#elif defined(__x86_64__)
  stats.vmcall_instructions = cpu->gstats.vmcall_instructions;
  stats.pause_instructions = cpu->gstats.pause_instructions;
  stats.xsetbv_instructions = cpu->gstats.xsetbv_instructions;
  stats.ept_violations = cpu->gstats.ept_violations;
  stats.wrmsr_instructions = cpu->gstats.wrmsr_instructions;
  stats.rdmsr_instructions = cpu->gstats.rdmsr_instructions;
  stats.io_instructions = cpu->gstats.io_instructions;
  stats.control_register_accesses = cpu->gstats.control_register_accesses;
  stats.hlt_instructions = cpu->gstats.hlt_instructions;
  stats.cpuid_instructions = cpu->gstats.cpuid_instructions;
  stats.interrupt_windows = cpu->gstats.interrupt_windows;
  stats.interrupts = cpu->gstats.interrupts;
#endif
  return stats;
}

zx_status_t actual_avail_result(size_t actual, size_t avail, user_out_ptr<size_t> user_actual,
                                user_out_ptr<size_t> user_avail) {
  if (user_actual) {
    zx_status_t status = user_actual.copy_to_user(actual);
    if (status != ZX_OK)
      return status;
  }
  if (user_avail) {
    zx_status_t status = user_avail.copy_to_user(avail);
    if (status != ZX_OK)
      return status;
  }
  return ZX_OK;
}

template <typename T>
zx_status_t single_record_result(user_out_ptr<void> dst_buffer, size_t dst_buffer_size,
                                 user_out_ptr<size_t> user_actual, user_out_ptr<size_t> user_avail,
                                 const T& src_record) {
  size_t actual = 1;
  if (dst_buffer_size >= sizeof(T)) {
    if (dst_buffer.reinterpret<T>().copy_to_user(src_record) != ZX_OK) {
      return ZX_ERR_INVALID_ARGS;
    }
  } else {
    actual = 0;
  }
  zx_status_t st = actual_avail_result(actual, 1, user_actual, user_avail);
  if (st != ZX_OK) {
    return st;
  }
  if (actual == 0)
    return ZX_ERR_BUFFER_TOO_SMALL;
  return ZX_OK;
}

// Copies to usermode an (fbl) array of results to |dst_buffer| up to |dst_buffer_size|. It uses
// actual_avail_result() to copy to usermode the available and actual copied records.
template <typename T>
zx_status_t multi_record_result(user_out_ptr<void> dst_buffer, size_t dst_buffer_size,
                                user_out_ptr<size_t> user_actual, user_out_ptr<size_t> user_avail,
                                const fbl::Array<T>& src_array) {
  size_t avail = src_array.size();
  size_t num_space_for = dst_buffer_size / sizeof(T);
  size_t actual = ktl::min(avail, num_space_for);
  // Don't try to copy if there are no bytes to copy, as the "is
  // user space" check may not handle (_buffer == NULL and len == 0).
  if (actual && dst_buffer.reinterpret<T>().copy_array_to_user(src_array.data(), actual) != ZX_OK) {
    return ZX_ERR_INVALID_ARGS;
  }
  return actual_avail_result(actual, avail, user_actual, user_avail);
}

template <zx_object_info_topic_t topic, typename TargetType>
auto ConvertInfoVersion(const TargetType& info) {
  return info;
}

template <>
auto ConvertInfoVersion<ZX_INFO_KMEM_STATS_EXTENDED>(const zx_info_kmem_stats_t& stats) {
  return KernelStatsInfoToVersion<zx_info_kmem_stats_extended_t>(stats);
}

template <>
auto ConvertInfoVersion<ZX_INFO_KMEM_STATS_V1>(const zx_info_kmem_stats_t& stats) {
  return KernelStatsInfoToVersion<zx_info_kmem_stats_v1>(stats);
}

template <>
auto ConvertInfoVersion<ZX_INFO_TASK_RUNTIME_V1>(const zx_info_task_runtime_t& info) {
  return zx_info_task_runtime_v1_t{
      .cpu_time = info.cpu_time,
      .queue_time = info.queue_time,
  };
}

template <>
auto ConvertInfoVersion<ZX_INFO_TASK_STATS_V1>(const zx_info_task_stats_t& info) {
  return zx_info_task_stats_v1_t{
      .mem_mapped_bytes = info.mem_mapped_bytes,
      .mem_private_bytes = info.mem_private_bytes,
      .mem_shared_bytes = info.mem_shared_bytes,
      .mem_scaled_shared_bytes = info.mem_scaled_shared_bytes,
  };
}

zx::result<uint64_t> GetClockMappedSize(ClockDispatcher* clock) {
  // Only mappable clocks have a defined mapped size.
  if (!clock->is_mappable()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(ClockDispatcher::kMappedSize);
}

zx::result<zx_info_task_stats_t> GetProcessStats(ProcessDispatcher* process) {
  zx_info_task_stats_t info = {};
  auto err = process->GetStats(&info);
  if (err != ZX_OK) {
    return zx::error(err);
  }
  return zx::ok(info);
}

zx::result<zx_info_process_handle_stats_t> GetHandleStats(ProcessDispatcher* process) {
  zx_info_process_handle_stats_t info = {};
  static_assert(ktl::size(info.handle_count) >= ZX_OBJ_TYPE_UPPER_BOUND,
                "Need room for each handle type.");

  process->handle_table().ForEachHandle(
      [&](zx_handle_t handle, zx_rights_t rights, const Dispatcher* dispatcher) {
        ++info.handle_count[dispatcher->get_type()];
        return ZX_OK;
      });
  return zx::ok(info);
}

// Vanilla version, takes a member function |mf| with no arguments which unconditionally
// returns the information.
#define OB_GET_INFO(id, Td, mf)                                                             \
  if constexpr (topic == (id)) {                                                            \
    fbl::RefPtr<Td> disp;                                                                   \
    zx_status_t status =                                                                    \
        up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &disp);   \
    if (status != ZX_OK) {                                                                  \
      return status;                                                                        \
    }                                                                                       \
    auto res = ConvertInfoVersion<id>(disp->mf());                                          \
    return single_record_result(dst_buffer, dst_buffer_size, user_actual, user_avail, res); \
  }

// Version that takes a function |fn| and validates the handle using the base |bs| resource
// of kind ZX_RSRC_KIND_SYSTEM.
#define OB_GET_INFO_SR(id, bs, fn)                                                          \
  if constexpr (topic == (id)) {                                                            \
    zx_status_t status = validate_ranged_resource(handle, ZX_RSRC_KIND_SYSTEM, (bs), 1);    \
    if (status != ZX_OK)                                                                    \
      return status;                                                                        \
    auto res = ConvertInfoVersion<id>(fn());                                                \
    return single_record_result(dst_buffer, dst_buffer_size, user_actual, user_avail, res); \
  }

// Version that takes a function-like |fn| which takes a T* and returns zx::result<I>.
#define OB_GET_INFO_ZR(id, Td, fn)                                                            \
  if constexpr (topic == (id)) {                                                              \
    fbl::RefPtr<Td> disp;                                                                     \
    zx_status_t status =                                                                      \
        up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &disp);     \
    if (status != ZX_OK) {                                                                    \
      return status;                                                                          \
    }                                                                                         \
    auto res = fn(disp.get());                                                                \
    if (res.is_error()) {                                                                     \
      return res.error_value();                                                               \
    }                                                                                         \
    auto res_v = ConvertInfoVersion<id>(res.value());                                         \
    return single_record_result(dst_buffer, dst_buffer_size, user_actual, user_avail, res_v); \
  }

// Starts a sequence of object-get-info with the same topic |id|.
#define OB_GET_INFO_BEGIN(id)                                                               \
  if constexpr (topic == (id)) {                                                            \
    {                                                                                       \
      constexpr int ID = id;                                                                \
      fbl::RefPtr<Dispatcher> disp;                                                         \
      zx_status_t status =                                                                  \
          up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &disp); \
      if (status != ZX_OK) {                                                                \
        return status;                                                                      \
      }

// Defines an entry within the OB_GET_INFO_BEGIN block, with |Td| being the required Dispatcher
// type.
#define OB_GET_INFO_EL(Td, mf)                                                                \
  {                                                                                           \
    auto actual_disp = DownCastDispatcher<Td>(&disp);                                         \
    if (actual_disp) {                                                                        \
      auto res = ConvertInfoVersion<ID>(actual_disp->mf());                                   \
      return single_record_result(dst_buffer, dst_buffer_size, user_actual, user_avail, res); \
    }                                                                                         \
  }

// Ends the sequence started by OB_GET_INFO_BEGIN.
#define OB_GET_INFO_END()   \
  return ZX_ERR_WRONG_TYPE; \
  }                         \
  }

template <zx_object_info_topic_t topic>
zx_status_t object_get_info_cpp(ProcessDispatcher* up, zx_handle_t handle,
                                user_out_ptr<void> dst_buffer, size_t dst_buffer_size,
                                user_out_ptr<size_t> user_actual, user_out_ptr<size_t> user_avail) {
  OB_GET_INFO_SR(ZX_INFO_KMEM_STATS, ZX_RSRC_SYSTEM_INFO_BASE, GetMemoryStats);
  OB_GET_INFO_SR(ZX_INFO_KMEM_STATS_EXTENDED, ZX_RSRC_SYSTEM_INFO_BASE, GetMemoryStats);
  OB_GET_INFO_SR(ZX_INFO_KMEM_STATS_V1, ZX_RSRC_SYSTEM_INFO_BASE, GetMemoryStats);
  OB_GET_INFO_SR(ZX_INFO_KMEM_STATS_COMPRESSION, ZX_RSRC_SYSTEM_INFO_BASE, GetCompressionStats);
  OB_GET_INFO_SR(ZX_INFO_MEMORY_STALL, ZX_RSRC_SYSTEM_STALL_BASE, GetStallStats);
  OB_GET_INFO(ZX_INFO_RESOURCE, ResourceDispatcher, GetInfo);
  OB_GET_INFO(ZX_INFO_STREAM, StreamDispatcher, GetInfo);
  OB_GET_INFO(ZX_INFO_VCPU, VcpuDispatcher, GetInfo);
  OB_GET_INFO(ZX_INFO_IOB, IoBufferDispatcher, GetInfo);
  OB_GET_INFO_ZR(ZX_INFO_CLOCK_MAPPED_SIZE, ClockDispatcher, GetClockMappedSize);
  OB_GET_INFO(ZX_INFO_INTERRUPT, InterruptDispatcher, GetInfo);

  OB_GET_INFO(ZX_INFO_PROCESS, ProcessDispatcher, GetInfo);
  OB_GET_INFO_ZR(ZX_INFO_TASK_STATS, ProcessDispatcher, GetProcessStats);
  OB_GET_INFO_ZR(ZX_INFO_TASK_STATS_V1, ProcessDispatcher, GetProcessStats);
  OB_GET_INFO_ZR(ZX_INFO_PROCESS_HANDLE_STATS, ProcessDispatcher, GetHandleStats);

  OB_GET_INFO_BEGIN(ZX_INFO_TASK_RUNTIME)
  OB_GET_INFO_EL(JobDispatcher, GetRuntimeStats)
  OB_GET_INFO_EL(ProcessDispatcher, GetRuntimeStats)
  OB_GET_INFO_EL(ThreadDispatcher, GetRuntimeStats)
  OB_GET_INFO_END()

  OB_GET_INFO_BEGIN(ZX_INFO_TASK_RUNTIME_V1)
  OB_GET_INFO_EL(JobDispatcher, GetRuntimeStats)
  OB_GET_INFO_EL(ProcessDispatcher, GetRuntimeStats)
  OB_GET_INFO_EL(ThreadDispatcher, GetRuntimeStats)
  OB_GET_INFO_END()

  return ZX_ERR_NOT_SUPPORTED;
}

}  // namespace

extern "C" {

zx_status_t cpp_object_get_info_cpp_types(zx_handle_t handle, uint32_t topic, void* _buffer,
                                          size_t buffer_size, size_t* _actual, size_t* _avail) {
  auto up = ProcessDispatcher::GetCurrent();
  user_out_ptr<void> dst_buffer(_buffer);
  user_out_ptr<size_t> actual(_actual);
  user_out_ptr<size_t> avail(_avail);

  switch (topic) {
    case ZX_INFO_HANDLE_VALID: {
      // This syscall + topic is excepted from the ZX_POL_BAD_HANDLE policy.
      fbl::RefPtr<Dispatcher> generic_dispatcher;
      return up->handle_table().GetDispatcherWithRightsNoPolicyCheck(handle, 0, &generic_dispatcher,
                                                                     nullptr);
    }
    case ZX_INFO_PROCESS:
      return object_get_info_cpp<ZX_INFO_PROCESS>(up, handle, dst_buffer, buffer_size, actual,
                                                  avail);
    case ZX_INFO_PROCESS_THREADS: {
      fbl::RefPtr<ProcessDispatcher> process;
      auto error =
          up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_ENUMERATE, &process);
      if (error != ZX_OK)
        return error;

      // Getting the list of threads is inherently racy (unless the caller has already stopped all
      // threads.
      fbl::Array<zx_koid_t> threads;
      zx_status_t status = process->GetThreads(&threads);
      if (status != ZX_OK)
        return status;

      return multi_record_result(dst_buffer, buffer_size, actual, avail, threads);
    }
    case ZX_INFO_TASK_STATS:
      return object_get_info_cpp<ZX_INFO_TASK_STATS>(up, handle, dst_buffer, buffer_size, actual,
                                                     avail);
    case ZX_INFO_TASK_STATS_V1:
      return object_get_info_cpp<ZX_INFO_TASK_STATS_V1>(up, handle, dst_buffer, buffer_size, actual,
                                                        avail);
    case ZX_INFO_TASK_RUNTIME:
      return object_get_info_cpp<ZX_INFO_TASK_RUNTIME>(up, handle, dst_buffer, buffer_size, actual,
                                                       avail);
    case ZX_INFO_TASK_RUNTIME_V1:
      return object_get_info_cpp<ZX_INFO_TASK_RUNTIME_V1>(up, handle, dst_buffer, buffer_size,
                                                          actual, avail);
    case ZX_INFO_PROCESS_HANDLE_STATS:
      return object_get_info_cpp<ZX_INFO_PROCESS_HANDLE_STATS>(up, handle, dst_buffer, buffer_size,
                                                               actual, avail);
    case ZX_INFO_HANDLE_TABLE: {
      fbl::RefPtr<ProcessDispatcher> process;
      auto error = up->handle_table().GetDispatcherWithRights(
          *up, handle, ZX_RIGHT_INSPECT | ZX_RIGHT_MANAGE_PROCESS | ZX_RIGHT_MANAGE_THREAD,
          &process);
      if (error != ZX_OK)
        return error;

      if (!dst_buffer && !avail && actual) {
        // Optimization for callers which call twice, the first time just to know the size.
        return actual.copy_to_user(static_cast<size_t>(up->handle_table().HandleCount()));
      }
      fbl::Array<zx_info_handle_extended_t> handle_info;
      zx_status_t status = process->handle_table().GetHandleInfo(&handle_info);
      if (status != ZX_OK)
        return status;

      return multi_record_result(dst_buffer, buffer_size, actual, avail, handle_info);
    }
    case ZX_INFO_VMAR_MAPS: {
      fbl::RefPtr<VmAddressRegionDispatcher> vmar;
      zx_status_t status =
          up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &vmar);
      if (status != ZX_OK) {
        return status;
      }

      SubsetVmarMapsInfoWriter<zx_info_maps_t> writer{dst_buffer.reinterpret<zx_info_maps_t>()};
      const size_t max_records = buffer_size / sizeof(zx_info_maps_t);
      size_t actual_records = 0;
      size_t avail_records = 0;
      status =
          GetVmarMaps(vmar->vmar().get(), writer, max_records, &actual_records, &avail_records);
      if (status != ZX_OK)
        return status;

      return actual_avail_result(actual_records, avail_records, actual, avail);
    }
    case ZX_INFO_PROCESS_MAPS_V1:
    case ZX_INFO_PROCESS_MAPS_V2:
    case ZX_INFO_PROCESS_MAPS: {
      fbl::RefPtr<ProcessDispatcher> process;
      zx_status_t status =
          up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &process);
      if (status != ZX_OK) {
        return status;
      }

      size_t count = 0;
      size_t avail_count = 0;

      if (topic == ZX_INFO_PROCESS_MAPS_V1) {
        SubsetVmarMapsInfoWriter<zx_info_maps_v1_t> writer{
            dst_buffer.reinterpret<zx_info_maps_v1_t>()};
        count = buffer_size / sizeof(zx_info_maps_v1_t);
        status = process->GetAspaceMaps(writer, count, &count, &avail_count);
      } else if (topic == ZX_INFO_PROCESS_MAPS_V2) {
        SubsetVmarMapsInfoWriter<zx_info_maps_v2_t> writer{
            dst_buffer.reinterpret<zx_info_maps_v2_t>()};
        count = buffer_size / sizeof(zx_info_maps_v2_t);
        status = process->GetAspaceMaps(writer, count, &count, &avail_count);
      } else {
        SubsetVmarMapsInfoWriter<zx_info_maps_t> writer{dst_buffer.reinterpret<zx_info_maps_t>()};
        count = buffer_size / sizeof(zx_info_maps_t);
        status = process->GetAspaceMaps(writer, count, &count, &avail_count);
      }
      zx_status_t copy_status = actual_avail_result(count, avail_count, actual, avail);
      return (copy_status != ZX_OK) ? copy_status : status;
    }
    case ZX_INFO_PROCESS_VMOS_V1:
    case ZX_INFO_PROCESS_VMOS_V2:
    case ZX_INFO_PROCESS_VMOS_V3:
    case ZX_INFO_PROCESS_VMOS: {
      fbl::RefPtr<ProcessDispatcher> process;
      zx_status_t status =
          up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &process);
      if (status != ZX_OK) {
        return status;
      }

      size_t count = 0;
      size_t avail_count = 0;

      if (topic == ZX_INFO_PROCESS_VMOS_V1) {
        SubsetVmoInfoWriter<zx_info_vmo_v1_t> writer{dst_buffer.reinterpret<zx_info_vmo_v1_t>()};
        count = buffer_size / sizeof(zx_info_vmo_v1_t);
        status = process->GetVmos(writer, count, &count, &avail_count);
      } else if (topic == ZX_INFO_PROCESS_VMOS_V2) {
        SubsetVmoInfoWriter<zx_info_vmo_v2_t> writer{dst_buffer.reinterpret<zx_info_vmo_v2_t>()};
        count = buffer_size / sizeof(zx_info_vmo_v2_t);
        status = process->GetVmos(writer, count, &count, &avail_count);
      } else if (topic == ZX_INFO_PROCESS_VMOS_V3) {
        SubsetVmoInfoWriter<zx_info_vmo_v3_t> writer{dst_buffer.reinterpret<zx_info_vmo_v3_t>()};
        count = buffer_size / sizeof(zx_info_vmo_v3_t);
        status = process->GetVmos(writer, count, &count, &avail_count);
      } else {
        SubsetVmoInfoWriter<zx_info_vmo_t> writer{dst_buffer.reinterpret<zx_info_vmo_t>()};
        count = buffer_size / sizeof(zx_info_vmo_t);
        status = process->GetVmos(writer, count, &count, &avail_count);
      }
      zx_status_t copy_status = actual_avail_result(count, avail_count, actual, avail);
      return (copy_status != ZX_OK) ? copy_status : status;
    }
    case ZX_INFO_GUEST_STATS: {
      zx_status_t status =
          validate_ranged_resource(handle, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_INFO_BASE, 1);
      if (status != ZX_OK)
        return status;

      size_t num_cpus = arch_max_num_cpus();
      size_t num_space_for = buffer_size / sizeof(zx_info_guest_stats_t);
      size_t num_to_copy = ktl::min(num_cpus, num_space_for);
      user_out_ptr<zx_info_guest_stats_t> guest_buf =
          dst_buffer.reinterpret<zx_info_guest_stats_t>();

      for (unsigned int i = 0; i < static_cast<unsigned int>(num_to_copy); i++) {
        zx_info_guest_stats_t stats = GetGuestCPUStats(i);
        if (guest_buf.copy_array_to_user(&stats, 1, i) != ZX_OK)
          return ZX_ERR_INVALID_ARGS;
      }
      return actual_avail_result(num_to_copy, num_cpus, actual, avail);
    }
    case ZX_INFO_CPU_STATS: {
      zx_status_t status =
          validate_ranged_resource(handle, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_INFO_BASE, 1);
      if (status != ZX_OK)
        return status;

      size_t num_cpus = arch_max_num_cpus();
      size_t num_space_for = buffer_size / sizeof(zx_info_cpu_stats_t);
      size_t num_to_copy = ktl::min(num_cpus, num_space_for);
      // build an alias to the output buffer that is in units of the cpu stat structure
      user_out_ptr<zx_info_cpu_stats_t> cpu_buf = dst_buffer.reinterpret<zx_info_cpu_stats_t>();

      for (unsigned int i = 0; i < static_cast<unsigned int>(num_to_copy); i++) {
        zx_info_cpu_stats_t stats = GetCPUStats(i);
        if (cpu_buf.copy_array_to_user(&stats, 1, i) != ZX_OK)
          return ZX_ERR_INVALID_ARGS;
      }
      return actual_avail_result(num_to_copy, num_cpus, actual, avail);
    }
    case ZX_INFO_KMEM_STATS:
      return object_get_info_cpp<ZX_INFO_KMEM_STATS>(up, handle, dst_buffer, buffer_size, actual,
                                                     avail);
    case ZX_INFO_KMEM_STATS_EXTENDED:
      return object_get_info_cpp<ZX_INFO_KMEM_STATS_EXTENDED>(up, handle, dst_buffer, buffer_size,
                                                              actual, avail);
    case ZX_INFO_KMEM_STATS_V1:
      return object_get_info_cpp<ZX_INFO_KMEM_STATS_V1>(up, handle, dst_buffer, buffer_size, actual,
                                                        avail);
    case ZX_INFO_KMEM_STATS_COMPRESSION:
      return object_get_info_cpp<ZX_INFO_KMEM_STATS_COMPRESSION>(up, handle, dst_buffer,
                                                                 buffer_size, actual, avail);
    case ZX_INFO_RESOURCE:
      return object_get_info_cpp<ZX_INFO_RESOURCE>(up, handle, dst_buffer, buffer_size, actual,
                                                   avail);
    case ZX_INFO_STREAM:
      return object_get_info_cpp<ZX_INFO_STREAM>(up, handle, dst_buffer, buffer_size, actual,
                                                 avail);
    case ZX_INFO_VCPU:
      return object_get_info_cpp<ZX_INFO_VCPU>(up, handle, dst_buffer, buffer_size, actual, avail);
    case ZX_INFO_IOB:
      return object_get_info_cpp<ZX_INFO_IOB>(up, handle, dst_buffer, buffer_size, actual, avail);
    case ZX_INFO_IOB_REGIONS: {
      fbl::RefPtr<IoBufferDispatcher> iob;
      zx_status_t status =
          up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_INSPECT, &iob);
      if (status != ZX_OK) {
        return status;
      }

      const size_t num_regions = iob->RegionCount();
      const size_t num_space_for = buffer_size / sizeof(zx_iob_region_info_t);
      const size_t num_to_copy = ktl::min(num_regions, num_space_for);

      for (size_t i = 0; i < num_to_copy; i++) {
        zx_iob_region_info_t region = iob->GetRegionInfo(i);
        status =
            dst_buffer.reinterpret<zx_iob_region_info_t>().element_offset(i).copy_to_user(region);
        if (status != ZX_OK) {
          return status;
        }
      }
      return actual_avail_result(num_to_copy, num_regions, actual, avail);
    }
    case ZX_INFO_MEMORY_STALL:
      return object_get_info_cpp<ZX_INFO_MEMORY_STALL>(up, handle, dst_buffer, buffer_size, actual,
                                                       avail);
    case ZX_INFO_CLOCK_MAPPED_SIZE:
      return object_get_info_cpp<ZX_INFO_CLOCK_MAPPED_SIZE>(up, handle, dst_buffer, buffer_size,
                                                            actual, avail);
    case ZX_INFO_INTERRUPT:
      return object_get_info_cpp<ZX_INFO_INTERRUPT>(up, handle, dst_buffer, buffer_size, actual,
                                                    avail);
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }
}

}  // extern "C"
