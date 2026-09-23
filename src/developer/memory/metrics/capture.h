// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_CAPTURE_H_
#define SRC_DEVELOPER_MEMORY_METRICS_CAPTURE_H_

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/handle.h>
#include <lib/zx/time.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <memory>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

#include <ffl/fixed.h>

#include "src/performance/memory/scudo/mallopt.h"

class TestMonitor;

namespace memory {

// TODO(https://fxbug.dev/338300808): Rename this to something more generic and incorporate it into
// ffl, as it duplicates code used inside the kernel.
struct FractionalBytes {
  using Fraction = ffl::Fixed<uint64_t, 63>;
  constexpr static Fraction kOneByte = Fraction(1);

  FractionalBytes operator+(const uint64_t& other) const {
    FractionalBytes ret{*this};
    ret += other;
    return ret;
  }
  FractionalBytes operator-(const uint64_t& other) const {
    FractionalBytes ret{*this};
    ret -= other;
    return ret;
  }
  FractionalBytes operator/(const uint64_t& other) const {
    FractionalBytes ret{*this};
    ret /= other;
    return ret;
  }
  FractionalBytes& operator+=(const uint64_t& other) {
    [[maybe_unused]] bool overflow = __builtin_add_overflow(integral, other, &integral);
    FX_DCHECK(!overflow);
    return *this;
  }
  FractionalBytes& operator-=(const uint64_t& other) {
    [[maybe_unused]] bool overflow = __builtin_sub_overflow(integral, other, &integral);
    FX_DCHECK(!overflow);
    return *this;
  }
  FractionalBytes& operator/=(const uint64_t& other) {
    // Input fraction must always be <1 to guard against overflow.
    // If this is true, the sum of fractions must be <1:
    // The sum is:
    //  `(fractional / other) + (1 / other) * remainder`
    // which we can rewrite as
    //  `(fractional + remainder) / other`
    // We know that fractional < 1 and remainder < other, thus (fractional + remainder) < other and
    // the rewritten sum cannot be >=1.
    FX_DCHECK(fractional < kOneByte);
    const uint64_t remainder = integral % other;
    const Fraction scaled_remainder = (kOneByte / other) * remainder;
    fractional = (fractional / other) + scaled_remainder;
    FX_DCHECK(fractional < kOneByte);
    integral /= other;

    return *this;
  }

  FractionalBytes operator+(const FractionalBytes& other) const {
    FractionalBytes ret{*this};
    ret += other;
    return ret;
  }
  FractionalBytes& operator+=(const FractionalBytes& other) {
    // Input fractions must always be <1 to guard against overflow.
    // If the fractional sum is >=1, then roll that overflow byte into the integral part.
    FX_DCHECK(fractional < kOneByte);
    FX_DCHECK(other.fractional < kOneByte);
    fractional += other.fractional;
    if (fractional >= kOneByte) {
      [[maybe_unused]] bool overflow = __builtin_add_overflow(integral, 1, &integral);
      FX_DCHECK(!overflow);
      fractional -= kOneByte;
    }
    [[maybe_unused]] bool overflow = __builtin_add_overflow(integral, other.integral, &integral);
    FX_DCHECK(!overflow);

    return *this;
  }

  bool operator<(const FractionalBytes& other) const {
    return integral < other.integral ||
           (integral == other.integral && fractional < other.fractional);
  }
  bool operator>(const FractionalBytes& other) const {
    return integral > other.integral ||
           (integral == other.integral && fractional > other.fractional);
  }
  bool operator<=(const FractionalBytes& other) const { return *this < other || *this == other; }
  bool operator>=(const FractionalBytes& other) const { return *this > other || *this == other; }
  bool operator==(const uint64_t& other) const {
    return integral == other && fractional == Fraction::FromRaw(0);
  }
  bool operator!=(const uint64_t& other) const { return !(*this == other); }
  bool operator==(const FractionalBytes& other) const {
    return integral == other.integral && fractional == other.fractional;
  }
  bool operator!=(const FractionalBytes& other) const { return !(*this == other); }

  uint64_t integral = 0;
  Fraction fractional = Fraction::FromRaw(0);
};

struct Process {
  zx_koid_t koid;
  zx_koid_t job;
  char name[ZX_MAX_NAME_LEN];
  std::vector<zx_koid_t> vmos;
};

struct Vmo {
  explicit Vmo(const zx_info_vmo_t& v)
      : koid(v.koid), parent_koid(v.parent_koid), allocated_bytes(v.size_bytes) {
    // Use the kernel's PSS value (i.e. `committed_scaled_bytes`) as it properly accounts for
    // copy-on-write sharing.
    committed_scaled_bytes = FractionalBytes{
        .integral = v.committed_scaled_bytes,
        .fractional = FractionalBytes::Fraction::FromRaw(v.committed_fractional_scaled_bytes)};
    populated_scaled_bytes = FractionalBytes{
        .integral = v.populated_scaled_bytes,
        .fractional = FractionalBytes::Fraction::FromRaw(v.populated_fractional_scaled_bytes)};
    strncpy(name, v.name, sizeof(name));
  }
  zx_koid_t koid;
  char name[ZX_MAX_NAME_LEN];
  zx_koid_t parent_koid;
  uint64_t allocated_bytes;
  FractionalBytes committed_scaled_bytes;
  FractionalBytes populated_scaled_bytes;
  std::vector<zx_koid_t> children;
};

enum class CaptureLevel : uint8_t { KMEM, PROCESS, VMO };

// OS is an abstract interface to Zircon OS calls.
class OS {
 public:
  virtual ~OS() = default;
  virtual zx_status_t GetKernelStats(fidl::WireSyncClient<fuchsia_kernel::Stats>* stats_client) = 0;
  virtual zx_handle_t ProcessSelf() = 0;
  virtual zx_instant_boot_t GetBoot() = 0;
  virtual zx_status_t GetProcesses(
      fit::function<zx_status_t(int /* depth */, zx::handle /* handle */, zx_koid_t /* koid */,
                                zx_koid_t /* parent_koid */)>
          cb) = 0;
  virtual zx_status_t GetProperty(zx_handle_t handle, uint32_t property, void* value,
                                  size_t name_len) = 0;
  virtual zx_status_t GetInfo(zx_handle_t handle, uint32_t topic, void* buffer, size_t buffer_size,
                              size_t* actual, size_t* avail) = 0;

  virtual zx_status_t GetKernelMemoryStats(
      const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
      zx_info_kmem_stats_t& kmem) = 0;
  virtual zx_status_t GetKernelMemoryStatsExtended(
      const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
      zx_info_kmem_stats_extended_t& kmem_ext, zx_info_kmem_stats_t* kmem) = 0;
  virtual zx_status_t GetKernelMemoryStatsCompression(
      const fidl::WireSyncClient<fuchsia_kernel::Stats>& stats_client,
      zx_info_kmem_stats_compression_t& kmem_compression) = 0;
};

// Returns an OS implementation querying Zircon Kernel.
std::unique_ptr<OS> CreateDefaultOS();

class MallocPurgeGuard {
 public:
  ~MallocPurgeGuard() { mallopt(M_PURGE, /*not used*/ 0); }
};

class Capture {
 public:
  static const std::unordered_set<std::string> kDefaultRootedVmoNames;

  zx_instant_boot_t time() const { return time_; }
  const zx_info_kmem_stats_t& kmem() const { return kmem_; }
  const std::optional<zx_info_kmem_stats_extended_t>& kmem_extended() const {
    return kmem_extended_;
  }
  const std::optional<zx_info_kmem_stats_compression_t>& kmem_compression() const {
    return kmem_compression_;
  }

  const std::unordered_map<zx_koid_t, Process>& koid_to_process() const { return koid_to_process_; }

  const std::unordered_map<zx_koid_t, Vmo>& koid_to_vmo() const { return koid_to_vmo_; }

  const Process& process_for_koid(zx_koid_t koid) const { return koid_to_process_.at(koid); }

  const Vmo& vmo_for_koid(zx_koid_t koid) const { return koid_to_vmo_.at(koid); }

 private:
  MallocPurgeGuard purge_guard_;
  zx_instant_boot_t time_;
  zx_info_kmem_stats_t kmem_ = {};
  std::optional<zx_info_kmem_stats_extended_t> kmem_extended_;
  std::optional<zx_info_kmem_stats_compression_t> kmem_compression_;
  std::unordered_map<zx_koid_t, Process> koid_to_process_;
  std::unordered_map<zx_koid_t, Vmo> koid_to_vmo_;
  std::vector<zx_koid_t> root_vmos_;

  friend class ::TestMonitor;
  friend class TestUtils;
  friend class CaptureMaker;
};

// Holds the necessary components required to create a |Capture|.
class CaptureMaker {
 public:
  static fit::result<zx_status_t, CaptureMaker> Create(std::unique_ptr<OS> os);

  zx_status_t GetCapture(
      Capture* capture, CaptureLevel level,
      const std::unordered_set<std::string>& rooted_vmo_names = Capture::kDefaultRootedVmoNames);

 private:
  CaptureMaker(fidl::WireSyncClient<fuchsia_kernel::Stats> stats_client, std::unique_ptr<OS> os);
  static void ReallocateDescendants(Vmo& parent, std::unordered_map<zx_koid_t, Vmo>& koid_to_vmo);
  static void ReallocateDescendants(const std::unordered_set<std::string>& rooted_vmo_names,
                                    std::unordered_map<zx_koid_t, Vmo>& koid_to_vmo);
  // zx_koid_t self_koid_;
  fidl::WireSyncClient<fuchsia_kernel::Stats> stats_client_;
  std::unique_ptr<OS> os_;

  friend class TestUtils;
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_CAPTURE_H_
