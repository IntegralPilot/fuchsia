// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/machine.h>
#include <lib/fit/function.h>
#include <lib/ld/testing/startup-ld-abi.h>
#include <lib/ld/tls.h>
#include <lib/zx/object.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <map>
#include <ranges>
#include <thread>

#include <zxtest/zxtest.h>

#include "../test/safe-zero-construction.h"
#include "stack-abi.h"
#include "thread-storage-test-utils.h"
#include "thread-storage.h"
#include "threads_impl.h"
#include "tls-dep.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

using TlsLayout = elfldltl::TlsLayout<>;
using TlsTraits = elfldltl::TlsTraits<>;

using InitializeTlsFn = void(std::span<std::byte> thread_block, size_t tp_offset);

class TemporaryVmarForTest {
 public:
  TemporaryVmarForTest() = default;
  TemporaryVmarForTest(TemporaryVmarForTest&&) = default;
  TemporaryVmarForTest& operator=(TemporaryVmarForTest&&) = default;

  explicit operator bool() const { return vmar_.is_valid(); }

  void Init(PageRoundedSize size) {
    ASSERT_FALSE(vmar_);
    uintptr_t base;
    EXPECT_OK(zx::vmar::root_self()->allocate(  //
        kTestVmarOptions, 0, size.get(), &vmar_, &base));
    size_ = size;
  }

  const zx::vmar& vmar() const { return vmar_; }

  PageRoundedSize size() const { return size_; }

  ~TemporaryVmarForTest() {
    if (vmar_) {
      EXPECT_OK(vmar_.destroy());
    }
  }

 private:
  static constexpr zx_vm_option_t kTestVmarOptions = ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE;

  zx::vmar vmar_;
  PageRoundedSize size_;
};

class LibcThreadTests : public ::zxtest::Test {
 public:
  // Place everything inside a constrained VMAR that's always destroyed at the
  // end of the test, just in case.
  static inline const PageRoundedSize kTestVmarSize = *PageRoundedSize::From(1 << 30);

  void SetUp() override {
    LibcThreadTestScopedTlsGlobals::gTlsLayout = {};
    LibcThreadTestScopedTlsGlobals::gInitializeTls = {};
  }

  // Get the test VMAR, setting it up if need be.
  zx::unowned_vmar TestVmar(PageRoundedSize size = kTestVmarSize) {
    if (size != test_vmar_.size()) {
      if (test_vmar_) {
        test_vmar_ = {};
      }
      test_vmar_.Init(size);
    }
    return test_vmar_.vmar().borrow();
  }

  thrd_zx_create_handles_t CreateHandles(PageRoundedSize size = kTestVmarSize) {
    zx::unowned_vmar vmar = TestVmar(size);
    return {
        .machine_stack_vmar = vmar->get(),
        .security_stack_vmar = vmar->get(),
        .thread_block_vmar = vmar->get(),
    };
  }

 private:
  TemporaryVmarForTest test_vmar_;
};

constexpr std::string_view kVmoName = "thread-storage-test";

const PageRoundedSize kOnePage = *PageRoundedSize::Pages(1);
const PageRoundedSize kManyPages = *PageRoundedSize::Pages(256);

constexpr size_t kStackCount = 1 + (kSafeStackAbi ? 1 : 0) + (kShadowCallStackAbi ? 1 : 0);

// This prevents the compiler from thinking it knows what the returned pointer
// is, so it must really do a load to read *ptr if that value is used; and must
// assume *ptr contains global state meaningful elsewhere and so actually do a
// store for `*ptr = ...`.
auto* Launder(auto* ptr) {
  __asm__ volatile("" : "=r"(ptr) : "0"(ptr));
  return ptr;
}

// Returns a death lambda for trying to read *ptr.
auto DeathByRead(const auto* ptr) {
  // The empty asm prevents the compiler from thinking it can elide the load
  // because it doesn't need the value, while Launder prevents it from finding
  // the value somewhere other than by doing that load.
  return [ptr] { __asm__ volatile("" : : "r"(*Launder(ptr))); };
}

// Returns a death lambda for trying to write *ptr.
auto DeathByWrite(auto* ptr, std::decay_t<decltype(*ptr)> value = {}) {
  return [ptr, value] { *Launder(ptr) = value; };
}

// There is never a `new Thread` actually done.  The memory is just zero-filled
// by the system, and then individual fields get set.  Ensure nobody can tell.
TEST_F(LibcThreadTests, SafeZeroConstruction) {
  LIBC_NAMESPACE::ExpectSafeZeroConstruction<Thread>();
}

TEST_F(LibcThreadTests, TpSelfPointer) {
  if constexpr (!TlsTraits::kTpSelfPointer) {
    ZXTEST_SKIP() << "no $tp -> self pointer in this machine's ABI";
    return;
  }

  // This is just testing the running system, not any code "under test".
  // But it verifies the TlsTraits expectation about the machine's ABI.
  const void* const* tp = ld::TpRelative<const void*>(0);
  EXPECT_EQ(tp, *tp);
}

// This expects everything except the Thread::abi slots to be all zero bytes.
void CheckZeroThread(const Thread* thread) {
  static constexpr std::array<std::byte, sizeof(Thread)> kZero{};

  Thread tcb;
  memcpy(&tcb, thread, sizeof(tcb));
  tcb.abi = {};
  if constexpr (TlsTraits::kTpSelfPointer) {
    tcb.head.tp = 0;
  }

  EXPECT_BYTES_EQ(&tcb, kZero.data(), kZero.size());
}

void CheckThread(Thread* thread) {
  // The Thread pointer is available.
  ASSERT_NE(thread, nullptr);
  CheckZeroThread(thread);

  if constexpr (TlsTraits::kTpSelfPointer) {
    // *$tp = $tp is already set.
    void* tp = pthread_to_tp(thread);
    EXPECT_EQ(tp, *Launder(ld::TpRelative<void*>(0, tp)))
        << "Thread @ " << static_cast<void*>(thread) << " -> tp=" << tp;
  }

  // The abi.stack_guard slot should start zero and be mutable.
  EXPECT_EQ(thread->abi.stack_guard, 0u);
  thread->abi.stack_guard = 0xdeadbeef;
  EXPECT_EQ(*Launder(&thread->abi.stack_guard), 0xdeadbeefu);
  EXPECT_EQ(ld::TpRelative<uintptr_t>(ZX_TLS_STACK_GUARD_OFFSET, pthread_to_tp(thread)),
            &thread->abi.stack_guard);
}

// A stack should be accessible and mutable within; guarded below if it grows
// down; guarded above if it grows up.
template <bool GrowsUp = false>
void CheckStack(std::string_view stack_name, PageRoundedSize stack_size, PageRoundedSize guard_size,
                uint64_t* sp, std::span<uint64_t> stack_span) {
  ASSERT_NE(sp, nullptr) << stack_name;

  EXPECT_EQ(stack_span.size_bytes(), stack_size.get()) << stack_name;
  if constexpr (GrowsUp) {
    EXPECT_EQ(sp, stack_span.data()) << stack_name;
  } else {
    EXPECT_EQ(sp, stack_span.data() + stack_span.size()) << stack_name;
  }

  const size_t stack_words = stack_size.get() / sizeof(uint64_t);
  std::span<uint64_t> stack{sp - (GrowsUp ? 0 : stack_words), stack_words};
  EXPECT_EQ(stack.data(), stack_span.data());
  EXPECT_EQ(stack.size(), stack_span.size());

  // If zero-initialized and mutable at both ends, probably in the middle too.
  // It could get slow to check every word or even every page.

  EXPECT_EQ(stack[0], 0u) << stack_name;
  *Launder(stack.data()) = 0x1234u;
  EXPECT_EQ(stack[0], 0x1234u) << stack_name;

  EXPECT_EQ(stack.back(), 0u) << stack_name;
  *Launder(&stack.back()) = 0x6789u;
  EXPECT_EQ(stack.back(), 0x6789u) << stack_name;

  // Likewise, if guards fault at both ends, proboably in the middle too.

  uint64_t* in_guard = Launder(GrowsUp ? sp + stack_words : sp - stack_words - 1);
  ASSERT_DEATH(DeathByRead(in_guard), "%s sp=%p in_guard=%p", std::string(stack_name).c_str(), sp,
               in_guard);
  ASSERT_DEATH(DeathByWrite(in_guard), "%s sp=%p in_guard=%p", std::string(stack_name).c_str(), sp,
               in_guard);

  const size_t guard_words = guard_size.get() / sizeof(uint64_t);
  uint64_t* far_in_guard =
      Launder(GrowsUp ? sp + stack_words + guard_words - 1 : stack.data() - guard_words);
  ASSERT_DEATH(DeathByRead(far_in_guard), "%s", std::string(stack_name).c_str());
  ASSERT_DEATH(DeathByWrite(far_in_guard), "%s", std::string(stack_name).c_str());
}

void CheckStorage(PageRoundedSize stack_size, PageRoundedSize guard_size,
                  const ThreadStorage& storage, Thread* thread) {
  CheckThread(thread);

  if constexpr (kSafeStackAbi) {
    // The abi.unsafe_sp slot should already be filled in.
    CheckStack("unsafe stack", stack_size, guard_size,
               reinterpret_cast<uint64_t*>(thread->abi.unsafe_sp), storage.unsafe_stack());
  } else {
    EXPECT_EQ(storage.unsafe_sp(), nullptr);
    EXPECT_TRUE(storage.unsafe_stack().empty());
    EXPECT_EQ(thread->abi.unsafe_sp, 0u);
  }

  CheckStack("machine stack", stack_size, guard_size, storage.machine_sp(),
             storage.machine_stack());

  if constexpr (kShadowCallStackAbi) {
    CheckStack<true>("shadow call stack", stack_size, guard_size, storage.shadow_call_sp(),
                     storage.shadow_call_stack());
  } else {
    EXPECT_EQ(storage.shadow_call_sp(), nullptr);
    EXPECT_TRUE(storage.shadow_call_stack().empty());
  }
}

void CheckVmoName(zx::unowned_vmar vmar, std::string_view expected_name, zx_vaddr_t vaddr) {
  // Auto-size the vector for zx_object_get_info.
  constexpr auto get_info = []<typename T>(auto&& handle, zx_object_info_topic_t topic,
                                           std::vector<T>& info) {
    while (true) {
      size_t actual = 0, avail = 0;
      zx_status_t status =
          handle->get_info(topic, info.data(), info.size() * sizeof(T), &actual, &avail);
      info.resize(avail);
      if (status != ZX_ERR_BUFFER_TOO_SMALL) {
        ASSERT_OK(status);
        ASSERT_LE(actual, avail);
        if (actual == avail) {
          return;
        }
      }
    }
  };

  // List all the mappings in the test VMAR.
  std::vector<zx_info_maps_t> maps;
  ASSERT_NO_FATAL_FAILURE(get_info(vmar->borrow(), ZX_INFO_VMAR_MAPS, maps));
  auto by_base = [](const zx_info_maps_t& info) { return info.base; };
  ASSERT_TRUE(std::ranges::is_sorted(maps, std::ranges::less{}, by_base));

  // List all the VMOs used in the process.
  std::vector<zx_info_vmo_t> vmos;
  ASSERT_NO_FATAL_FAILURE(get_info(zx::process::self(), ZX_INFO_PROCESS_VMOS, vmos));

  // Put the VMO info into a map indexed by KOID.
  auto vmos_view = std::views::transform(
      std::views::all(vmos), [](const zx_info_vmo_t& info) -> std::pair<zx_koid_t, zx_info_vmo_t> {
        return {info.koid, info};
      });
  std::map<zx_koid_t, zx_info_vmo_t> vmos_by_koid(vmos_view.begin(), vmos_view.end());

  // Find the mapping covering the vaddr.  It has a KOID for the VMO it maps.
  auto maps_view = std::views::all(maps);
  auto last = std::ranges::upper_bound(maps_view, vaddr, std::ranges::less{}, by_base);
  ASSERT_NE(last, maps_view.begin());
  --last;
  ASSERT_EQ(last->type, ZX_INFO_MAPS_TYPE_MAPPING)
      << std::hex << std::showbase << vaddr << " not found, last at " << last->base;

  auto vmo = vmos_by_koid.find(last->u.mapping.vmo_koid);
  ASSERT_NE(vmo, vmos_by_koid.end());
  const zx_info_vmo_t& vmo_info = vmo->second;
  std::string_view vmo_name{vmo_info.name, std::size(vmo_info.name)};
  vmo_name = vmo_name.substr(0, vmo_name.find_first_of('\0'));
  EXPECT_STREQ(std::string{expected_name}, std::string{vmo_name});
}

TEST_F(LibcThreadTests, ThreadStorage) {
  ThreadStorage storage;

  // Empty when constructed.
  EXPECT_EQ(storage.stack_size().get(), 0u);
  EXPECT_EQ(storage.guard_size().get(), 0u);
  EXPECT_EQ(storage.machine_sp(), nullptr);
  EXPECT_EQ(storage.unsafe_sp(), nullptr);
  EXPECT_EQ(storage.shadow_call_sp(), nullptr);

  EXPECT_TRUE(storage.machine_stack().empty());
  EXPECT_TRUE(storage.unsafe_stack().empty());
  EXPECT_TRUE(storage.shadow_call_stack().empty());

  // Allocate the most basic layout: one-page stacks, one-page guards.
  auto result = storage.Allocate(CreateHandles(), kVmoName, kOnePage, kOnePage);
  ASSERT_TRUE(result.is_ok()) << result.status_string();

  CheckVmoName(TestVmar(), kVmoName, reinterpret_cast<uintptr_t>(*result));

  CheckStorage(kOnePage, kOnePage, storage, *result);
}

TEST_F(LibcThreadTests, ThreadStorageTooBig) {
  ThreadStorage storage;

  // Use a stack size so big that they can't all be mapped in.
  const PageRoundedSize stack{kTestVmarSize / 2};
  auto result = storage.Allocate(CreateHandles(), kVmoName, stack, kOnePage);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.error_value(), ZX_ERR_NO_RESOURCES);
}

TEST_F(LibcThreadTests, ThreadStorageBigStack) {
  ThreadStorage storage;

  auto result = storage.Allocate(CreateHandles(), kVmoName, kManyPages, kOnePage);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  CheckStorage(kManyPages, kOnePage, storage, *result);
}

TEST_F(LibcThreadTests, ThreadStorageBigGuard) {
  ThreadStorage storage;

  auto result = storage.Allocate(CreateHandles(), kVmoName, kOnePage, kManyPages);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  CheckStorage(kOnePage, kManyPages, storage, *result);
}

TEST_F(LibcThreadTests, ThreadStorageNoGuard) {
  constexpr PageRoundedSize kNoGuard{};

  ThreadStorage storage;

  // Use a tiny test VMAR that only has space for the requested sizes.  If all
  // the blocks fit, then there can't be any guard pages.  Each stack is one
  // page with no guards.  The thread block always gets two one-page guards, so
  // the minimal one is three pages.
  const PageRoundedSize vmar_size = *PageRoundedSize::Pages(kStackCount + 3);
  auto result = storage.Allocate(CreateHandles(vmar_size), kVmoName, kOnePage, kNoGuard);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
}

TEST_F(LibcThreadTests, ThreadStorageTls) {
  // Use a trivial TLS layout as if one TLS module has one uint32_t variable.
  constexpr size_t kTlsStart = TlsTraits::kTlsLocalExecOffset;
  static constexpr TlsLayout kTrivialLayout{
      kTlsStart + sizeof(uint32_t),
      sizeof(uint32_t),
  };
  constexpr ptrdiff_t kTlsBias =
      static_cast<ptrdiff_t>(kTlsStart) -
      (TlsTraits::kTlsNegative ? static_cast<ptrdiff_t>(kTrivialLayout.size_bytes()) : 0);

  LibcThreadTestScopedTlsGlobals::gTlsLayout = kTrivialLayout;

  // This both initializes that "variable" and checks that it all started
  // zero-initialized so InitializeTls doesn't need to zero the tbss space.
  constexpr uint32_t kInitValue = 123467890;
  LibcThreadTestScopedTlsGlobals::gInitializeTls = [](std::span<std::byte> thread_block,
                                                      size_t tp_offset) {
    ASSERT_LT(tp_offset + kTlsBias, thread_block.size_bytes())
        << " tp_offset " << tp_offset << " + bias " << kTlsBias;
    std::span segment = thread_block.subspan(tp_offset + kTlsBias, sizeof(uint32_t));
    ASSERT_EQ(segment.size_bytes(), sizeof(uint32_t));

    // The segment should be zero-initialized.
    uint32_t* ptr = reinterpret_cast<uint32_t*>(segment.data());
    EXPECT_EQ(*Launder(ptr), 0u);

    *Launder(ptr) = kInitValue;
  };

  ThreadStorage storage;
  auto result = storage.Allocate(CreateHandles(), kVmoName, kOnePage, kOnePage);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  CheckStorage(kOnePage, kOnePage, storage, *result);

  void* tp = pthread_to_tp(*result);
  uint32_t* ptr = ld::TpRelative<uint32_t>(kTlsBias, tp);
  EXPECT_EQ(*Launder(ptr), kInitValue) << "\n    TLS initial data from $tp " << tp << " + bias "
                                       << kTlsBias << " = " << static_cast<void*>(ptr);
  ++*ptr;
  EXPECT_EQ(*Launder(ptr), kInitValue + 1) << "\n    TLS mutated data from $tp " << tp << " + bias "
                                           << kTlsBias << " = " << static_cast<void*>(ptr);
}

TEST_F(LibcThreadTests, ThreadStorageTlsAlignment) {
  // Use a layout with the largest supported alignment requirement: one page.
  const TlsLayout kBigAlignmentLayout{17, kOnePage.get()};
  const auto aligned_big = [kBigAlignmentLayout](size_t size) -> size_t {
    return size == 0 ? 0 : kBigAlignmentLayout.Align(size);
  };
  const size_t kAlignedSize = aligned_big(kBigAlignmentLayout.size_bytes());

  LibcThreadTestScopedTlsGlobals::gTlsLayout = kBigAlignmentLayout;
  LibcThreadTestScopedTlsGlobals::gInitializeTls =
      [tls_bias =  // Compute the $tp bias for the first module.
       static_cast<ptrdiff_t>(aligned_big(TlsTraits::kTlsLocalExecOffset)) -
       static_cast<ptrdiff_t>(TlsTraits::kTlsNegative ? kAlignedSize : 0)](
          std::span<std::byte> thread_block, size_t tp_offset) {
        uintptr_t tp = reinterpret_cast<uintptr_t>(thread_block.data() + tp_offset);
        EXPECT_EQ(0u, (tp + tls_bias) % kOnePage.get())
            << std::hex << std::showbase << "\n    $tp " << tp << " from ["
            << static_cast<void*>(thread_block.data()) << ","
            << static_cast<void*>(thread_block.data() + thread_block.size()) << ") + " << tp_offset
            << "\n        + TLS bias " << tls_bias << " = " << tp + tls_bias
            << "\n    not aligned to " << kOnePage.get();
      };

  ThreadStorage storage;
  auto result = storage.Allocate(CreateHandles(), kVmoName, kOnePage, kOnePage);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  CheckStorage(kOnePage, kOnePage, storage, *result);
}

TEST_F(LibcThreadTests, ThreadStorageTlsReal) {
  // Use some real TLS data from the executable and an Initial Exec module to
  // ensure things match what the real compiled TLS accesses resolved to and
  // the other tests aren't just matching bugs with the implementation.
  constinit thread_local uint64_t localexec_initial_data = 0x12346789abcdef;
  const ptrdiff_t kLeOffset = ld::TpRelativeToOffset(&localexec_initial_data);
  const ptrdiff_t kIeOffset = ld::TpRelativeToOffset(&tls_dep_data);

  LibcThreadTestScopedTlsGlobals::gTlsLayout = ld::testing::gStartupLdAbi.static_tls_layout;
  LibcThreadTestScopedTlsGlobals::gInitializeTls = [](std::span<std::byte> thread_block,
                                                      size_t tp_offset) {
    ld::TlsInitialExecDataInit(ld::testing::gStartupLdAbi, thread_block, tp_offset, true);
  };

  auto check_tls = [this, kLeOffset, kIeOffset] {
    // Check basic assumptions about the ambient program state first.
    ASSERT_EQ(*Launder(&localexec_initial_data), 0x12346789abcdef);
    ASSERT_EQ(*Launder(&tls_dep_data), kTlsDepDataValue);
    ASSERT_EQ(*Launder(&tls_dep_bss[0]), '\0');
    ASSERT_EQ(*Launder(&tls_dep_bss[1]), '\0');
    ASSERT_EQ(kLeOffset, ld::TpRelativeToOffset(&localexec_initial_data));
    ASSERT_EQ(kIeOffset, ld::TpRelativeToOffset(&tls_dep_data));

    ASSERT_GE(LibcThreadTestScopedTlsGlobals::gTlsLayout.size_bytes(),
              std::abs(kIeOffset) + sizeof(uint32_t));

    ThreadStorage storage;
    auto result = storage.Allocate(CreateHandles(), kVmoName, kOnePage, kOnePage);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    CheckStorage(kOnePage, kOnePage, storage, *result);

    void* tp = pthread_to_tp(*result);
    EXPECT_EQ(*Launder(ld::TpRelative<uint64_t>(kLeOffset, tp)), 0x12346789abcdef);
    EXPECT_EQ(*Launder(ld::TpRelative<int>(kIeOffset, tp)), kTlsDepDataValue);
    EXPECT_EQ(*Launder(ld::TpRelative<char>(kIeOffset + sizeof(uint32_t), tp)), '\0');
    EXPECT_EQ(*Launder(ld::TpRelative<char>(kIeOffset + sizeof(uint32_t) + 1, tp)), '\0');
  };

  // Do the same check on the initial thread and on a second thread just to be
  // sure the test's own expectations really make sense.
  ASSERT_NO_FATAL_FAILURE(check_tls());
  std::jthread from_other_thread(check_tls);
}

decltype(auto) operator<<(auto& os, std::span<zx_info_maps_t> info) {
  os << "{";
  for (const auto& entry : info) {
    os << "{\"" << entry.name << "\", " << std::hex << std::showbase << entry.base << ", "
       << entry.size << ", type=" << entry.type << "}\n";
  }
  return os << "}";
}

// Each VMAR should contain a child VMAR that in turn contains just one mapping
// that is exactly the block.
void CheckVmar(std::string_view which, zx::unowned_vmar vmar, std::span<const std::byte> block) {
  std::array<zx_info_maps_t, 8> info;
  size_t actual, avail;
  ASSERT_OK(vmar->get_info(ZX_INFO_VMAR_MAPS, info.data(), sizeof(info), &actual, &avail)) << which;
  ASSERT_EQ(avail, actual) << which;

  std::stringstream s;
  s << std::span{info}.subspan(0, actual);
  ASSERT_EQ(actual, 3u) << which << ": " << s.str();

  EXPECT_EQ(info[0].type, ZX_INFO_MAPS_TYPE_VMAR) << which << ": " << s.str();
  EXPECT_EQ(info[1].type, ZX_INFO_MAPS_TYPE_VMAR) << which << ": " << s.str();
  EXPECT_EQ(info[2].type, ZX_INFO_MAPS_TYPE_MAPPING) << which << ": " << s.str();

  const uintptr_t block_base = reinterpret_cast<uintptr_t>(block.data());
  EXPECT_EQ(info[2].base, block_base) << which;
  EXPECT_EQ(info[2].size, block.size_bytes()) << which;
}

TEST_F(LibcThreadTests, ThreadStorageCreateHandles) {
  TemporaryVmarForTest machine_stack, security_stack, thread_block;
  ASSERT_NO_FATAL_FAILURE(machine_stack.Init(kManyPages));
  ASSERT_NO_FATAL_FAILURE(security_stack.Init(kManyPages));
  ASSERT_NO_FATAL_FAILURE(thread_block.Init(kManyPages));

  const thrd_zx_create_handles_t handles = {
      .machine_stack_vmar = machine_stack.vmar().get(),
      .security_stack_vmar = security_stack.vmar().get(),
      .thread_block_vmar = thread_block.vmar().get(),
  };

  ThreadStorage storage;
  auto result = storage.Allocate(handles, kVmoName, kOnePage, kOnePage);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  CheckStorage(kOnePage, kOnePage, storage, *result);

  CheckVmar("thread block", thread_block.vmar().borrow(), storage.thread_block());
  CheckVmar("machine stack", machine_stack.vmar().borrow(), std::as_bytes(storage.machine_stack()));
  if constexpr (kShadowCallStackAbi) {
    CheckVmar("shadow call stack", security_stack.vmar().borrow(),
              std::as_bytes(storage.shadow_call_stack()));
  }
  if constexpr (kSafeStackAbi) {
    CheckVmar("unsafe stack", security_stack.vmar().borrow(),
              std::as_bytes(storage.unsafe_stack()));
  }
}

void CheckThreadBlockForLayout(const TlsLayout& layout, thrd_zx_create_handles_t handles) {
  LibcThreadTestScopedTlsGlobals::gTlsLayout = layout;

  std::span<std::byte> found_thread_block;
  size_t found_tp_offset;
  LibcThreadTestScopedTlsGlobals::gInitializeTls =  //
      [&found_thread_block, &found_tp_offset](std::span<std::byte> thread_block, size_t tp_offset) {
        // Just record the resulting thread block calculations.
        found_thread_block = thread_block;
        found_tp_offset = tp_offset;
      };

  ThreadStorage storage;
  auto result = storage.Allocate(handles, kVmoName, kOnePage, kOnePage);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  CheckStorage(kOnePage, kOnePage, storage, *result);

  // The entire block should be valid. CheckStorage eventually checks the TCB portion.
  // The TLS portion should be readable and zero.
  const ptrdiff_t kTlsBias =
      static_cast<ptrdiff_t>(TlsTraits::kTlsLocalExecOffset) -
      (TlsTraits::kTlsNegative ? static_cast<ptrdiff_t>(layout.size_bytes()) : 0);
  std::span<std::byte> tls_portion =
      found_thread_block.subspan(found_tp_offset + kTlsBias, layout.size_bytes());
  EXPECT_TRUE(std::all_of(tls_portion.begin(), tls_portion.end(),
                          [](std::byte b) { return b == std::byte{0}; }));

  EXPECT_EQ(ld::TpRelative(-found_tp_offset, pthread_to_tp(*result)), found_thread_block.data())
      << "Expected the tp offset to point to the start of the thread block";

  ASSERT_EQ(found_thread_block.size() % kOnePage.get(), 0);
  EXPECT_GE(found_thread_block.size(), layout.size_bytes())
      << "Insufficient block size to hold the full TLS layout";
}

// This is a minimal reproducer for the issue at https://fxbug.dev/492277399.
// Prior, this used to page fault when accessing dtv[1] due to the VMO for
// the thread block being too small and accessing a guard page. This is a
// regression test using the specific TLS values found in the bug.
//
// TODO(https://fxbug.dev/496386493): We should refactor the other
// TLS layout cases into a generalized table-driven way that allows running
// a set of layouts over the same tests.
TEST_F(LibcThreadTests, ThreadStorage_b492277399_RegressionTest) {
  // This is the layout used which led to the issue described in
  // https://fxbug.dev/492277399.
  constexpr TlsLayout kInterestingLayout{
      6992,
      32,
  };
  CheckThreadBlockForLayout(kInterestingLayout, CreateHandles());

  // 1232 is sizeof(Thread) at the time of writing this test. sizeof(Thread)
  // can change over time which affects arithmetic and different rounding
  // effects which may uncover new bugs, so this test may capture then as
  // Thread changes.
  constexpr TlsLayout kInterestingLayoutThreadSize{
      6992 - 1232 + sizeof(Thread),
      32,
  };
  CheckThreadBlockForLayout(kInterestingLayoutThreadSize, CreateHandles());
}

TEST_F(LibcThreadTests, ThreadStorageStackTooSmall) {
  constexpr size_t kStackSizes[] = {0, PTHREAD_STACK_MIN - 1};

  for (size_t stack_size : kStackSizes) {
    const PageRoundedSize rounded_stack = *PageRoundedSize::From(stack_size);
    if (rounded_stack.get() >= PTHREAD_STACK_MIN) {
      continue;
    }

    ThreadStorage storage;
    auto result = storage.Allocate(CreateHandles(), kVmoName, rounded_stack, kOnePage);
    EXPECT_EQ(result.status_value(), ZX_ERR_INVALID_ARGS)
        << "Expected ZX_ERR_INVALID_ARGS for stack_size: " << stack_size;
    ;
  }
}

TEST_F(LibcThreadTests, ThreadStorageSizeLargerThanVmar) {
  ThreadStorage storage;
  const PageRoundedSize vmar_size = *PageRoundedSize::Pages(10);
  const PageRoundedSize large_stack = *PageRoundedSize::Pages(20);
  auto result = storage.Allocate(CreateHandles(vmar_size), kVmoName, large_stack, kOnePage);
  EXPECT_EQ(result.status_value(), ZX_ERR_NO_RESOURCES);
}

TEST_F(LibcThreadTests, SizeOverflow) {
  // If adding any sizes together would result in an overflow, then it should be
  // treated as if we ran out of space in the allocating VMAR since an overflow
  // means we couldn't possibly fit this total size into the VMAR.
  // Use sizes that are valid but sum to overflow.
  const auto stack_size_opt = PageRoundedSize::From(0x8000000000000000);
  const auto guard_size_opt = PageRoundedSize::From(0x8000000000000000);
  ASSERT_TRUE(stack_size_opt);
  ASSERT_TRUE(guard_size_opt);
  const PageRoundedSize stack_size_rounded = *stack_size_opt;
  const PageRoundedSize guard_size_rounded = *guard_size_opt;

  ThreadStorage thread_storage;
  auto result =
      thread_storage.Allocate(CreateHandles(), kVmoName, stack_size_rounded, guard_size_rounded);

  EXPECT_EQ(result.error_value(), ZX_ERR_NO_RESOURCES);
}

}  // namespace

}  // namespace LIBC_NAMESPACE_DECL
