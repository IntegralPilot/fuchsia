// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_CLOCK_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_CLOCK_DISPATCHER_H_

#include <lib/fasttime/clock.h>
#include <lib/page/size.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/syscalls/clock.h>
#include <zircon/types.h>

#include <object/dispatcher.h>
#include <object/handle.h>

namespace internal {
struct FuchsiaKernelOsal;
}

class ClockDispatcher final : public SoloDispatcher<ClockDispatcher, ZX_DEFAULT_CLOCK_RIGHTS> {
 public:
  static inline constexpr uint64_t kMappedSize = kPageSize;

  static zx_status_t Create(uint64_t options, const zx_clock_create_args_v1_t& create_args,
                            KernelHandle<ClockDispatcher>* handle, zx_rights_t* rights);

  ~ClockDispatcher() final;
  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_CLOCK; }
  [[nodiscard]] zx_status_t get_name(char (&out_name)[ZX_MAX_NAME_LEN]) const final;
  [[nodiscard]] zx_status_t set_name(const char* name, size_t len) final;

  zx_status_t Read(zx_time_t* out_now);
  zx_status_t GetDetails(zx_clock_details_v1_t* out_details);

  template <typename UpdateArgsType>
  zx_status_t Update(uint64_t options, const UpdateArgsType& args);

  const fbl::RefPtr<VmObjectPaged>& vmo() { return vmo_; }
  bool is_mappable() const { return clock_transformation_->is_mappable(); }

 private:
  ClockDispatcher(uint64_t options, zx_time_t backstop_time, fbl::RefPtr<VmObjectPaged> vmo);

  // Supply the kernel specific implementation of ArchYield and reference timer
  // accessors in order to use libfasttime's implementation of the
  // ClockTransformation class.
  struct ClockTransformationAdapter {
    static inline void ArchYield() { arch::Yield(); }
    static zx_instant_mono_ticks_t GetMonoTicks() { return current_mono_ticks(); }
    static zx_instant_boot_ticks_t GetBootTicks() { return current_boot_ticks(); }
  };

  static zx_time_t GetCurrentTime(bool boot_time) {
    return boot_time ? current_boot_time() : current_mono_time();
  }

  using ClockTransformationType = ::fasttime::ClockTransformation<ClockTransformationAdapter>;
  static_assert(sizeof(ClockTransformationType) <= kMappedSize);
  static_assert(alignof(ClockTransformationType) <= kMappedSize);

  alignas(ClockTransformationType) uint8_t local_storage_[sizeof(ClockTransformationType)];
  const fbl::RefPtr<VmObjectPaged> vmo_;
  ClockTransformationType* clock_transformation_{nullptr};

  fbl::Name<ZX_MAX_NAME_LEN> name_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_CLOCK_DISPATCHER_H_
