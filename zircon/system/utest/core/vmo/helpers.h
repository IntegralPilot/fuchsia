// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_VMO_HELPERS_H_
#define ZIRCON_SYSTEM_UTEST_CORE_VMO_HELPERS_H_

#include <lib/fit/defer.h>
#include <lib/zx/bti.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <cstdint>
#include <vector>

#include <zxtest/zxtest.h>

namespace vmo_test {

inline void VmoWrite(const zx::vmo& vmo, uint32_t data, uint64_t offset = 0) {
  zx_status_t status = vmo.write(static_cast<void*>(&data), offset, sizeof(data));
  ASSERT_OK(status, "write failed");
}

inline uint32_t VmoRead(const zx::vmo& vmo, uint64_t offset = 0) {
  uint32_t val = 0;
  zx_status_t status = vmo.read(&val, offset, sizeof(val));
  EXPECT_OK(status, "read failed");
  return val;
}

inline void VmoCheck(const zx::vmo& vmo, uint32_t expected, uint64_t offset = 0) {
  uint32_t data;
  zx_status_t status = vmo.read(static_cast<void*>(&data), offset, sizeof(data));
  ASSERT_OK(status, "read failed");
  ASSERT_EQ(expected, data);
}

// Creates a vmo with |page_count| pages and writes (page_index + 1) to each page.
inline void InitPageTaggedVmo(uint32_t page_count, zx::vmo* vmo) {
  zx_status_t status;
  status = zx::vmo::create(page_count * zx_system_get_page_size(), ZX_VMO_RESIZABLE, vmo);
  ASSERT_OK(status, "create failed");
  for (unsigned i = 0; i < page_count; i++) {
    ASSERT_NO_FATAL_FAILURE(VmoWrite(*vmo, i + 1, i * zx_system_get_page_size()));
  }
}

// Repeatedly poll VMO until |predicate| returns true or an error occurs.
//
// Returns true on success, false on error.
//
// |Predicate| is a function that accepts |const zx_info_vmo_t&| and returns |bool|.
template <typename Predicate>
bool PollVmoInfoUntil(const zx::vmo& vmo, Predicate&& predicate) {
  zx_info_vmo_t info;
  while (true) {
    if (vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr) != ZX_OK) {
      return false;
    }
    if (predicate(info)) {
      return true;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(50)));
  }
}

inline size_t VmoNumChildren(const zx::vmo& vmo) {
  zx_info_vmo_t info;
  if (vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr) != ZX_OK) {
    return UINT64_MAX;
  }
  return info.num_children;
}

// Repeatedly poll |vmo| until the |expected_num_children| is observed.
//
// Returns true on success, false on error.
inline bool PollVmoNumChildren(const zx::vmo& vmo, size_t expected_num_children) {
  return PollVmoInfoUntil(vmo, [&](const zx_info_vmo_t& info) {
    if (info.num_children == expected_num_children) {
      return true;
    }
    printf("polling again. actual num children %zu; expected num children %zu\n", info.num_children,
           expected_num_children);
    return false;
  });
}

inline size_t VmoPopulatedBytes(const zx::vmo& vmo) {
  zx_info_vmo_t info;
  if (vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr) != ZX_OK) {
    return UINT64_MAX;
  }
  return info.populated_fractional_scaled_bytes == UINT64_MAX ? info.populated_bytes
                                                              : info.populated_scaled_bytes;
}

inline size_t VmoPopulatedFractionalBytes(const zx::vmo& vmo) {
  zx_info_vmo_t info;
  if (vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr) != ZX_OK) {
    return UINT64_MAX;
  }
  return info.populated_fractional_scaled_bytes;
}

// Repeatedly poll |vmo| until the |expected_populated_bytes| is observed.
//
// Returns true on success, false on error.
inline bool PollVmoPopulatedBytes(const zx::vmo& vmo, size_t expected_populated_bytes) {
  return PollVmoInfoUntil(vmo, [&](const zx_info_vmo_t& info) {
    if (info.populated_fractional_scaled_bytes == UINT64_MAX) {
      if (info.populated_bytes == expected_populated_bytes) {
        return true;
      }
      printf("polling again. actual bytes %zu (%zu pages); expected bytes %zu (%zu pages)\n",
             info.populated_bytes, info.populated_bytes / zx_system_get_page_size(),
             expected_populated_bytes, expected_populated_bytes / zx_system_get_page_size());
    } else {
      if (info.populated_scaled_bytes == expected_populated_bytes) {
        return true;
      }
      printf("polling again. actual bytes %zu (%zu pages); expected bytes %zu (%zu pages)\n",
             info.populated_scaled_bytes, info.populated_scaled_bytes / zx_system_get_page_size(),
             expected_populated_bytes, expected_populated_bytes / zx_system_get_page_size());
    }
    return false;
  });
}

// Create a fit::defer which will check a BTI to make certain that it has no
// pinned or quarantined pages when it goes out of scope, and fail the test if
// it does.
inline auto CreateDeferredBtiCheck(const zx::bti& bti) {
  return fit::defer([&bti]() {
    if (bti.is_valid()) {
      zx_info_bti_t info;
      ASSERT_OK(bti.get_info(ZX_INFO_BTI, &info, sizeof(info), nullptr, nullptr));
      EXPECT_EQ(0, info.pmo_count);
      EXPECT_EQ(0, info.quarantine_count);
    }
  });
}

// Simple class for managing vmo mappings w/o any external dependencies.
class Mapping {
 public:
  ~Mapping() {
    if (addr_) {
      ZX_ASSERT(zx::vmar::root_self()->unmap(addr_, len_) == ZX_OK);
    }
  }

  zx_status_t Init(const zx::vmo& vmo, size_t len) {
    zx_status_t status =
        zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, len, &addr_);
    len_ = len;
    return status;
  }

  uint32_t* ptr() { return reinterpret_cast<uint32_t*>(addr_); }
  uint8_t* bytes() { return reinterpret_cast<uint8_t*>(addr_); }

 private:
  uint64_t addr_ = 0;
  size_t len_ = 0;
};

// A simple struct and function which can be used to attempt to fetch a VMO
// created using zx_vmo_create_physical from a region which should have been
// reserved using the kernel.test.ram.reserve boot option.
struct PhysVmo {
  uintptr_t addr = 0;
  size_t size = 0;
  zx::vmo vmo;
};

// Create and return a physical VMO from the reserved regions of RAM.  |size|
// indicates the desired size of the VMO, or 0 to fetch the entire reserved
// region of RAM, whatever its size might be.
zx::result<PhysVmo> GetTestPhysVmo(size_t size = 0);

zx::bti CreateNamedBti(const zx::iommu& fake_iommu, uint32_t options, uint64_t bti_id,
                       const char* name);

// There are a few tests in this suite which attempt to perform a _large_ number
// of iterations of the test, typically looking for something like a race
// condition regression.  This can lead to problems in some worst case
// scenarios.  If the test is running in non KVM assisted emulation (as it would
// on RISC-V, currently), and the test harness machine is very overloaded
// (something which does happen, unfortunately), it is possible for a test to
// not be able to perform its 1000 (for example) iterations before timing out,
// even if everything is working correctly.
//
// Since these tests tend to be looking for non-deterministic repros of races in
// the case of regression, there really is no good number of iterations to pick
// here.  1000 is a lot, but it does not mean that the test is guaranteed to
// catch a regression (there is no number large enough to guarantee that).  This
// said, in worst case scenarios, the test can end up timing out and generating
// flake.
//
// So, add a small helper class in an attempt to balance these two issues.  We'd
// _like_ to run the test through X cycles, but if it is taking longer than Y
// second to do so, we should probably simply print a warning call the test
// done early.  This way, we are still getting a lot of iterations in CI/CQ, but
// hopefully not causing any false positive flake when things are not running
// quickly in the test environment.
//
class TestLimiter {
 public:
  TestLimiter(uint32_t iterations, zx::duration time_limit)
      : iterations_{iterations}, time_limit_{time_limit} {}
  ~TestLimiter() {}

  TestLimiter(const TestLimiter&) = delete;
  TestLimiter& operator=(const TestLimiter&) = delete;
  TestLimiter(TestLimiter&&) = delete;
  TestLimiter& operator=(TestLimiter&&) = delete;

  uint32_t iteration() const { return iteration_; }
  void next() { ++iteration_; }

  bool Finished() const {
    if (iteration_ >= iterations_) {
      return true;
    }

    zx::duration test_time = zx::clock::get_monotonic() - start_time_;
    if (test_time >= time_limit_) {
      printf("\nWARNING - Things seem to be running slowly, exiting test early.\n");
      printf("%u/%u iterations were successfully completed in ~%lu mSec.\n", iteration_,
             iterations_, test_time.to_msecs());
      return true;
    }

    return false;
  }

 private:
  uint32_t iteration_{0};
  const uint32_t iterations_;
  const zx::duration time_limit_;
  const zx::time_monotonic start_time_{zx::clock::get_monotonic()};
};

// For the given number of pages and fill byte value, return a
// permanently-usable, read-only buffer.  The first call for each page count
// and byte value will allocate and fill the buffer, and subsequent calls will
// just reuse the same one.  All these buffers live forever once they are used,
// but there is a limited number of PageCount, FillByte combinations used in
// each test and the page counts are not huge.
template <size_t PageCount, uint8_t FillByte = 0>
inline std::span<const uint8_t> TestFillPages() {
  static const std::vector<uint8_t> kData(PageCount * zx_system_get_page_size(), FillByte);
  return std::span{kData};
}

}  // namespace vmo_test

#endif  // ZIRCON_SYSTEM_UTEST_CORE_VMO_HELPERS_H_
