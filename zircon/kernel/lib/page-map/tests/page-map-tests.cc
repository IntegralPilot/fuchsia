// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page-map.h>
#include <lib/unittest/unittest.h>

namespace {

// A helper that creates a VMO of a given size.
zx::result<fbl::RefPtr<VmObjectPaged>> MakeVmo(size_t size) {
  static constexpr uint32_t kVmoOptions = 0;
  static constexpr uint32_t kPmmAllocFlags = PMM_ALLOC_FLAG_ANY | PMM_ALLOC_FLAG_CAN_WAIT;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(kPmmAllocFlags, kVmoOptions, size, &vmo);
  if (status != ZX_OK) {
    return zx::error_result(status);
  }
  return zx::ok(ktl::move(vmo));
}

bool PageMapCtorDtorTest() {
  BEGIN_TEST;

  page_map::PageMap pm1;
  page_map::PageMap pm2;

  END_TEST;
}

struct SomeStruct {
  uint32_t field_a;
  uint16_t field_b;
  uint16_t field_c;
};

bool PageMapMakeAccessorTest() {
  BEGIN_TEST;

  // Happy case.
  {
    zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
    ASSERT_TRUE(vmo.is_ok());
    page_map::PageMap pm;
    auto result = pm.MakeAccessor<SomeStruct>(ktl::move(vmo.value()), 0);
    ASSERT_TRUE(result.is_ok());
  }

  // Zero size VMO fails.
  {
    zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(0);
    ASSERT_TRUE(vmo.is_ok());
    page_map::PageMap pm;
    auto result = pm.MakeAccessor<SomeStruct>(ktl::move(vmo.value()), 0);
    ASSERT_TRUE(result.is_error());
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
  }

  // Offset out of range fails.
  {
    zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
    ASSERT_TRUE(vmo.is_ok());
    page_map::PageMap pm;

    // Not out of range.
    {
      auto result = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
      ASSERT_TRUE(result.is_ok());
    }

    // Just beyond.
    {
      auto result = pm.MakeAccessor<SomeStruct>(vmo.value(), kPageSize);
      ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
    }

    // Way out there.
    {
      auto result = pm.MakeAccessor<SomeStruct>(vmo.value(), 1000 * kPageSize);
      ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
    }

    // Trigger integer overflow.
    {
      auto result = pm.MakeAccessor<SomeStruct>(vmo.value(), ktl::numeric_limits<size_t>::max());
      ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
    }

    // Trigger integer overflow for any possible mapping base address.
    {
      auto result = pm.MakeAccessor<SomeStruct>(
          vmo.value(), ktl::numeric_limits<size_t>::max() - (16 * kPageSize) + 1);
      ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
    }

    // Stradles page boundary.
    {
      auto result = pm.MakeAccessor<SomeStruct>(vmo.value(), kPageSize - (sizeof(SomeStruct) / 2));
      ASSERT_EQ(ZX_ERR_INVALID_ARGS, result.error_value());
    }

    // Invalid alignment.
    {
      auto result = pm.MakeAccessor<SomeStruct>(vmo.value(), 1);
      ASSERT_EQ(ZX_ERR_INVALID_ARGS, result.error_value());
    }
  }

  // Simple VMO reuse.
  //
  // Create two accessors for the same VMO.  See that changes made to one are visible in the other.
  {
    zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
    ASSERT_TRUE(vmo.is_ok());
    page_map::PageMap pm;

    // Two accessors for the same object.
    auto a1 = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
    ASSERT_TRUE(a1.is_ok());
    ASSERT_TRUE(a1->IsValid());
    auto a2 = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
    ASSERT_TRUE(a2.is_ok());
    ASSERT_TRUE(a2->IsValid());

    SomeStruct v1 = {.field_a = 1u};
    a1->Write(v1);

    SomeStruct v2 = {.field_a = 0u};
    a2->Read(v2);
    ASSERT_EQ(v2.field_a, 1u);

    v2.field_a = 42u;
    a2->Write(v2);

    a1->Read(v1);
    ASSERT_EQ(v1.field_a, 42u);
  }

  END_TEST;
}

bool PageMapFieldAccessorTest() {
  BEGIN_TEST;

  // See when modifying a field of an object, only that field is modified.

  zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
  ASSERT_TRUE(vmo.is_ok());
  page_map::PageMap pm;

  // Two accessors for the same object.
  auto accessor = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
  ASSERT_TRUE(accessor.is_ok());
  ASSERT_TRUE(accessor->IsValid());

  // Set an initial value for the whole object.
  const SomeStruct initial_value = {.field_a = 1u, .field_b = 2, .field_c = 3};
  accessor->Write(initial_value);
  SomeStruct v;
  accessor->Read(v);
  ASSERT_BYTES_EQ(reinterpret_cast<uint8_t*>(&v), reinterpret_cast<const uint8_t*>(&initial_value),
                  sizeof(v));

  // Write one field.
  uint32_t a = 6;
  accessor->Write<&SomeStruct::field_a>(a);

  // See that the other fields are unaffected.
  uint16_t x;
  accessor->Read<&SomeStruct::field_b>(x);
  ASSERT_EQ(x, initial_value.field_b);
  accessor->Read<&SomeStruct::field_c>(x);
  ASSERT_EQ(x, initial_value.field_c);

  END_TEST;
}

bool AccessorMoveTest() {
  BEGIN_TEST;

  // See that move-assignment invalidates the source.
  {
    zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
    ASSERT_TRUE(vmo.is_ok());
    page_map::PageMap pm;
    auto source = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
    ASSERT_TRUE(source.is_ok());
    ASSERT_TRUE(source->IsValid());

    auto destination = ktl::move(source);

    ASSERT_TRUE(destination->IsValid());
    ASSERT_FALSE(source->IsValid());
  }

  // See that move-constructor invalidates the source.
  {
    zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
    ASSERT_TRUE(vmo.is_ok());
    page_map::PageMap pm;
    auto source = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
    ASSERT_TRUE(source.is_ok());
    ASSERT_TRUE(source->IsValid());

    auto destination(ktl::move(source));

    ASSERT_TRUE(destination->IsValid());
    ASSERT_FALSE(source->IsValid());
  }

  END_TEST;
}

bool LastAccessorDestroysVmoTest() {
  BEGIN_TEST;

  // Create a VMO and see that its ref-count is one.
  zx::result<fbl::RefPtr<VmObjectPaged>> vmo = MakeVmo(kPageSize);
  ASSERT_TRUE(vmo.is_ok());
  EXPECT_EQ(1, vmo.value()->ref_count_debug());

  page_map::PageMap pm;

  // Create an accessor and see that the VMO's ref-count is at least two.  Remember the observed
  // ref-count.
  auto a1 = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
  ASSERT_TRUE(a1.is_ok());
  ASSERT_TRUE(a1->IsValid());
  const int observed = vmo.value()->ref_count_debug();
  EXPECT_GE(observed, 2);

  // Create a second accessor and see that the ref-count is unchanged.
  auto a2 = pm.MakeAccessor<SomeStruct>(vmo.value(), 0);
  ASSERT_TRUE(a2.is_ok());
  ASSERT_TRUE(a2->IsValid());
  EXPECT_EQ(vmo.value()->ref_count_debug(), observed);

  // Destroy one accessor.  Ref-count is unchanged.
  *a1 = page_map::Accessor<SomeStruct>{};
  EXPECT_EQ(vmo.value()->ref_count_debug(), observed);

  // Destroy the last accessor.  Ref-count is back to one (our original |vmo| RefPtr).
  *a2 = page_map::Accessor<SomeStruct>{};
  EXPECT_EQ(1, vmo.value()->ref_count_debug());

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(mapping_registry_tests)
UNITTEST("PageMapCtorDtorTest", PageMapCtorDtorTest)
UNITTEST("PageMapMakeAccessorTest", PageMapMakeAccessorTest)
UNITTEST("PageMapFieldAccessorTest", PageMapFieldAccessorTest)
UNITTEST("AccessorMoveTest", AccessorMoveTest)
UNITTEST("LastAccessorDestroysVmoTest", LastAccessorDestroysVmoTest)
UNITTEST_END_TESTCASE(mapping_registry_tests, "page-map", "page-map tests")
