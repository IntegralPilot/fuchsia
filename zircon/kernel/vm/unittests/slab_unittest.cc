// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page/size.h>

#include <ktl/memory.h>
#include <vm/page_slab_allocator.h>

#include "test_helper.h"

#include <ktl/enforce.h>

namespace vm_unittest {

namespace {

struct TestObject {
  uint64_t data[32];
};
constexpr size_t kObjectsPerSlab = kPageSize / sizeof(TestObject);

class TestSlabAllocator final : public PageSlabAllocator<sizeof(TestObject)> {
 public:
  ~TestSlabAllocator() { PageSlabAllocator<sizeof(TestObject)>::DebugFreeAllSlabs(); }

  size_t SlabsAllocated() const { return allocated_; }
  size_t SlabsFreed() const { return freed_; }
  size_t ActiveSlabs() const { return allocated_ - freed_; }

 private:
  vm_page_t* AllocSlab() override {
    vm_page_t* slab = PageSlabAllocator<sizeof(TestObject)>::AllocSlab();
    if (slab) {
      allocated_++;
    }
    return slab;
  }
  void FreeSlab(vm_page_t* slab) override {
    PageSlabAllocator<sizeof(TestObject)>::FreeSlab(slab);
    freed_++;
  }

  size_t allocated_ = 0;
  size_t freed_ = 0;
};

bool slab_smoke_test() {
  BEGIN_TEST;

  PageSlabAllocator<sizeof(TestObject)> alloc;
  TestObject* obj = alloc.allocate_object<TestObject>();
  ktl::construct_at(obj);
  ASSERT_NONNULL(obj);
  ktl::destroy_at(obj);
  alloc.deallocate_bytes(obj);

  END_TEST;
}

class TestRand {
 public:
  using result_type = uint32_t;
  explicit TestRand(uint32_t init) : state_(init) {}
  uint32_t operator()() { return rand(); }

  uint32_t rand() {
    state_ = test_rand(state_);
    return state_;
  }
  static constexpr uint32_t min() { return 0; }
  static constexpr uint32_t max() { return UINT32_MAX; }

 private:
  uint32_t state_;
};

bool slab_return_slab_test() {
  BEGIN_TEST;

  TestSlabAllocator alloc;

  // Allocate one slab worth of objects.
  for (size_t i = 0; i < kObjectsPerSlab; i++) {
    ASSERT_NONNULL(alloc.allocate_bytes());
  }
  EXPECT_EQ(alloc.ActiveSlabs(), 1u);

  // Allocate a second slab worth of objects.
  TestObject* objects[kObjectsPerSlab];
  for (size_t i = 0; i < kObjectsPerSlab; i++) {
    objects[i] = alloc.allocate_object<TestObject>();
    ASSERT_NONNULL(objects[i]);
    ktl::construct_at(objects[i]);
  }
  EXPECT_EQ(alloc.ActiveSlabs(), 2u);

  // Allocate into a third slab.
  ASSERT_NONNULL(alloc.allocate_bytes());
  EXPECT_EQ(alloc.ActiveSlabs(), 3u);

  // Free all the objects on the second slab and ensure the slab was returned.
  for (size_t i = 0; i < kObjectsPerSlab; i++) {
    EXPECT_EQ(alloc.ActiveSlabs(), 3u);
    ktl::destroy_at(objects[i]);
    alloc.deallocate_bytes(objects[i]);
  }
  EXPECT_EQ(alloc.ActiveSlabs(), 2u);

  END_TEST;
}

bool slab_no_leak_test() {
  BEGIN_TEST;

  // Continually allocate and free a range of objects, ensuring that the number of slabs allocated
  // at one time is never more than required to store all the objects.
  constexpr size_t kNumObjects = 400;
  constexpr size_t kMaxSlabs = TestSlabAllocator::SlabsRequired(kNumObjects);

  TestSlabAllocator alloc;
  size_t allocated = 0;
  TestObject* objects[kNumObjects] = {nullptr};

  TestRand r(42);

  for (size_t iter = 0; iter < 10000; iter++) {
    // Allocate some number of objects.
    size_t to_alloc = r.rand() % (kNumObjects - allocated);
    for (size_t i = 0; i < to_alloc; i++) {
      objects[allocated] = alloc.allocate_object<TestObject>();
      ASSERT_NONNULL(objects[allocated]);
      ktl::construct_at(objects[allocated]);
      allocated++;
    }
    EXPECT_LE(alloc.ActiveSlabs(), kMaxSlabs);

    // Randomize the objects.
    ktl::shuffle(&objects[0], &objects[allocated], r);

    // Free some subset if there are any allocations.
    size_t to_free = allocated > 0 ? r.rand() % allocated : 0;
    for (size_t i = 0; i < to_free; i++) {
      allocated--;
      ktl::destroy_at(objects[allocated]);
      alloc.deallocate_bytes(objects[allocated]);
    }
  }

  // Free any that are left.
  for (size_t i = 0; i < allocated; i++) {
    ktl::destroy_at(objects[i]);
    alloc.deallocate_bytes(objects[i]);
  }

  EXPECT_EQ(alloc.ActiveSlabs(), 0u);

  END_TEST;
}

bool slab_max_id_test() {
  BEGIN_TEST;

  // To satisfy the requirement that the number of objects completely fill any slabs we need to
  // cause sufficient top level slabs to allocated and used. Every top level slab requires an
  // allocation in the bottom level slab. To completely fill one bottom level slab then requires
  // kPageSize / sizeof(vm_page_t*) top level slabs, each requiring kPageSize / sizeof(TestObject)
  // objects to fill.
  // For this test we want enough objects for two bottom level slabs.
  constexpr uint32_t kNumObjects =
      (kPageSize * 2 / sizeof(vm_page_t*)) * (kPageSize / sizeof(TestObject));

  IdSlabAllocator<sizeof(TestObject), kNumObjects> alloc;

  // Allocate all the objects we are supposed to be able to have.
  for (uint32_t i = 0; i < kNumObjects; i++) {
    void* object = alloc.allocate_bytes();
    ASSERT_NONNULL(object);
    uint32_t id = alloc.AllocToId(object);
    EXPECT_LT(id, kNumObjects);
  }

  // Attempting another allocation should fail, as we are out of IDs.
  {
    TestObject* object = alloc.allocate_object<TestObject>();
    EXPECT_NULL(object);
  }

  // Free all the objects.
  for (uint32_t i = 0; i < kNumObjects; i++) {
    void* object = alloc.IdToAlloc(i);
    alloc.deallocate_object(object);
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(slab_tests)
VM_UNITTEST(slab_smoke_test)
VM_UNITTEST(slab_return_slab_test)
VM_UNITTEST(slab_no_leak_test)
VM_UNITTEST(slab_max_id_test)
UNITTEST_END_TESTCASE(slab_tests, "slab", "Slab allocator tests")

}  // namespace vm_unittest
