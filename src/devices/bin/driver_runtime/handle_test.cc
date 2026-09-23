// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "handle.h"

#include <lib/zx/event.h>

#include <set>
#include <vector>

#include <zxtest/zxtest.h>

namespace driver_runtime {

class FakeObject : public Object {
 public:
  ~FakeObject() {}
};

class HandleTest : public zxtest::Test {
 protected:
  HandleTest() {}

  void TearDown() override { ASSERT_EQ(0, driver_runtime::gHandleTableArena.num_allocated()); }
};

TEST_F(HandleTest, HandleExists) {
  auto object = fbl::AdoptRef(new FakeObject());
  auto handle_owner = Handle::Create(std::move(object));
  ASSERT_NOT_NULL(handle_owner);

  fdf_handle_t handle_value = handle_owner->handle_value();
  EXPECT_NE(handle_value, ZX_HANDLE_INVALID);

  EXPECT_TRUE(Handle::HandleExists(handle_value));
}

TEST_F(HandleTest, GetObject) {
  auto object = fbl::AdoptRef(new FakeObject());
  FakeObject* object_ptr = object.get();

  auto handle_owner = Handle::Create(std::move(object));
  ASSERT_NOT_NULL(handle_owner);

  fdf_handle_t handle_value = handle_owner->handle_value();
  EXPECT_NE(handle_value, ZX_HANDLE_INVALID);

  fbl::RefPtr<FakeObject> downcasted_object;
  EXPECT_OK(Handle::GetObject<FakeObject>(handle_value, &downcasted_object));
  EXPECT_EQ(downcasted_object.get(), object_ptr);
}

TEST_F(HandleTest, GetObjectTakeHandleOwnership) {
  auto object = fbl::AdoptRef(new FakeObject());
  FakeObject* object_ptr = object.get();

  auto handle_owner = Handle::Create(std::move(object));
  ASSERT_NOT_NULL(handle_owner);

  fdf_handle_t handle_value = handle_owner->handle_value();
  EXPECT_NE(handle_value, ZX_HANDLE_INVALID);

  // Drop ownership of the handle without deleting it.
  handle_owner.release();

  fbl::RefPtr<FakeObject> downcasted_object;
  EXPECT_OK(Handle::GetObject<FakeObject>(handle_value, &downcasted_object));
  EXPECT_EQ(downcasted_object.get(), object_ptr);

  // Re-take ownership of the handle.
  auto [taken_handle_owner, taken_object] = gHandleTableArena.TakeOwnership(handle_value);
  EXPECT_NOT_NULL(taken_handle_owner);
  EXPECT_EQ(taken_object.get(), object_ptr);
  handle_owner = std::move(taken_handle_owner);
}

TEST_F(HandleTest, GetDeletedHandle) {
  auto object = fbl::AdoptRef(new FakeObject());

  auto handle_owner = Handle::Create(std::move(object));
  ASSERT_NOT_NULL(handle_owner);

  fdf_handle_t handle_value = handle_owner->handle_value();
  EXPECT_NE(handle_value, ZX_HANDLE_INVALID);

  // Drop the handle.
  handle_owner = nullptr;

  // Create a new handle. It'll likely be using the just
  // freed handle.
  auto object2 = fbl::AdoptRef(new FakeObject());

  auto handle_owner2 = Handle::Create(std::move(object2));
  ASSERT_NOT_NULL(handle_owner2);

  fdf_handle_t handle_value2 = handle_owner2->handle_value();
  EXPECT_NE(handle_value2, ZX_HANDLE_INVALID);
  EXPECT_NE(handle_value2, handle_value);

  // The handle should be deleted.
  EXPECT_FALSE(Handle::HandleExists(handle_value));
  // Check we can correctly get the newly created handle.
  EXPECT_TRUE(Handle::HandleExists(handle_value2));
}

TEST_F(HandleTest, IsFdfHandle) {
  auto object = fbl::AdoptRef(new FakeObject());

  auto handle_owner = Handle::Create(std::move(object));
  ASSERT_NOT_NULL(handle_owner);

  fdf_handle_t handle_value = handle_owner->handle_value();
  EXPECT_NE(handle_value, FDF_HANDLE_INVALID);

  EXPECT_TRUE(Handle::IsFdfHandle(handle_value));

  EXPECT_TRUE(Handle::IsFdfHandle(FDF_HANDLE_INVALID));

  // Check a zircon handle.
  zx::event event;
  zx::event::create(0, &event);
  EXPECT_FALSE(Handle::IsFdfHandle(event.get()));
}

TEST_F(HandleTest, AllocateMax) {
  std::set<fdf_handle_t> allocated_handles;
  std::vector<HandleOwner> handles;
  for (size_t i = 0; i < HandleTableArena::kMaxNumHandles; i++) {
    auto object = fbl::AdoptRef(new FakeObject());
    auto handle_owner = Handle::Create(std::move(object));
    ASSERT_NOT_NULL(handle_owner);

    fdf_handle_t handle_value = handle_owner->handle_value();
    EXPECT_EQ(allocated_handles.find(handle_value), allocated_handles.end());
    allocated_handles.insert(handle_value);

    handles.push_back(std::move(handle_owner));
  }
  // No handles should be left.
  auto object = fbl::AdoptRef(new FakeObject());
  auto handle_owner = Handle::Create(std::move(object));
  ASSERT_NULL(handle_owner);

  // Free a handle and try to allocate again.
  handles.pop_back();
  object = fbl::AdoptRef(new FakeObject());
  handle_owner = Handle::Create(std::move(object));
  ASSERT_NOT_NULL(handle_owner);
  // The handle value should be different.
  fdf_handle_t handle_value = handle_owner->handle_value();
  EXPECT_EQ(allocated_handles.find(handle_value), allocated_handles.end());
}

}  // namespace driver_runtime
