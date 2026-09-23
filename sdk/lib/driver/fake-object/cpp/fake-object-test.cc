// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-object/cpp/fake-object.h>
#include <lib/driver/fake-object/cpp/internal.h>
#include <lib/zx/event.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/limits.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <array>
#include <memory>
#include <thread>
#include <utility>

#include <zxtest/zxtest.h>

namespace fake_object::test {

class FakeObjectTest : public zxtest::Test {
 protected:
  // Catch handles leaked through tests to ensure the library itself doesn't leak any.
  void TearDown() final { ASSERT_EQ(0, FakeHandleTable().size()); }

  zx::result<zx_koid_t> GetFakeObjectKoid(zx_handle_t handle) {
    zx::result result = FakeHandleTable().Get(handle);
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::success(result->get_koid());
  }
};

// By default a base |Object| should return ZX_ERR_NOT_SUPPORTED for
// all intercepted object syscalls. This tests that the dispatch for
// the fake syscall routing works for syscalls other tests don't exercise.
// They are organized into individual tests to make it easier to tell if a
// specific syscall is broken.
TEST_F(FakeObjectTest, ShimGetInfo) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_get_info(res.value(), 0, nullptr, 0, nullptr, nullptr),
                ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimGetProperty) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_get_property(res.value(), 0, nullptr, 0), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimSetProfile) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_set_profile(res.value(), 0, 0), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimSetProperty) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_set_property(res.value(), 0, nullptr, 0), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimSignal) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_signal(res.value(), 0, 0), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimSignalPeer) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_signal_peer(res.value(), 0, 0), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimWaitOne) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_wait_one(res.value(), 0, 0, nullptr), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, ShimWaitAsync) {
  zx::result res = CreateFakeObject();
  ASSERT_OK(res.status_value());
  ASSERT_STATUS(zx_object_wait_one(res.value(), 0, 0, nullptr), ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_close(res.value()));
}

TEST_F(FakeObjectTest, HandleValidityCheck) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(0, 0, &vmo));
  ASSERT_FALSE(FakeHandleTable::IsValidFakeHandle(vmo.get()));

  auto result = CreateFakeObject();
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(FakeHandleTable::IsValidFakeHandle(result.value()));
  EXPECT_OK(zx_handle_close(result.value()));
}

TEST_F(FakeObjectTest, Get) {
  EXPECT_EQ(FakeHandleTable().size(), 0);
  zx::result<zx_handle_t> obj = CreateFakeObject();
  EXPECT_OK(obj.status_value());

  zx::result<std::shared_ptr<FakeObject>> getter = FakeHandleTable().Get(obj.value());
  EXPECT_TRUE(getter.is_ok());

  EXPECT_EQ(FakeHandleTable().size(), 1);
  EXPECT_OK(zx_handle_close(obj.value()));
}

TEST_F(FakeObjectTest, DuplicateHandle) {
  // Setup, create a fake bti, make sure it is valid:
  zx::result<zx_handle_t> obj = CreateFakeObject();
  EXPECT_OK(obj.status_value());

  // Duplicate the handle, make sure it is valid and the same object:
  zx_handle_t obj_dup = ZX_HANDLE_INVALID;
  EXPECT_OK(zx_handle_duplicate(obj.value(), 0, &obj_dup));
  EXPECT_EQ(2, FakeHandleTable().size());
  zx::result obj_koid = GetFakeObjectKoid(obj.value());
  zx::result obj_dup_koid = GetFakeObjectKoid(obj_dup);
  EXPECT_OK(obj_koid.status_value());
  EXPECT_OK(obj_dup_koid.status_value());
  EXPECT_EQ(obj_koid.value(), obj_dup_koid.value());

  EXPECT_OK(zx_handle_close(obj.value()));
  EXPECT_OK(zx_handle_close(obj_dup));
  EXPECT_EQ(0u, FakeHandleTable().size());
}

TEST_F(FakeObjectTest, DuplicateRealHandle) {
  // Setup, create an event and duplicate it, to make sure that still works:
  zx::event event, event_dup;
  ASSERT_OK(zx::event::create(0u, &event), "Error during event create");
  EXPECT_OK(event.duplicate(ZX_RIGHT_SAME_RIGHTS, &event_dup));

  // The ZX_EVENT_SIGNALED bit is guaranteed to be 0 when we create the object.
  // Now signal the original event:
  ASSERT_OK(event.signal(0u, ZX_EVENT_SIGNALED));
  zx_signals_t pending;
  // Now wait for that signal on the duplicated version:
  EXPECT_OK(event_dup.wait_one(ZX_EVENT_SIGNALED, zx::time(0), &pending));
  EXPECT_EQ(pending & ZX_EVENT_SIGNALED, ZX_EVENT_SIGNALED, "Error during wait call");
}

TEST_F(FakeObjectTest, ReplaceHandle) {
  zx::result<zx_handle_t> obj = CreateFakeObject();
  zx_handle_t obj_repl_hnd = ZX_HANDLE_INVALID;

  EXPECT_OK(obj.status_value());
  zx::result<zx_koid_t> original_koid = GetFakeObjectKoid(obj.value());
  EXPECT_OK(original_koid.status_value());
  EXPECT_OK(zx_handle_replace(obj.value(), 0, &obj_repl_hnd));
  EXPECT_STATUS(FakeHandleTable().Get(obj.value()).status_value(), ZX_ERR_NOT_FOUND);
  EXPECT_EQ(original_koid, GetFakeObjectKoid(obj_repl_hnd));

  EXPECT_OK(zx_handle_close(obj_repl_hnd));
  EXPECT_EQ(0u, FakeHandleTable().size());
}

TEST_F(FakeObjectTest, ReplaceRealHandle) {
  zx::event event, event_repl;
  ASSERT_OK(zx::event::create(0u, &event), "Error during event create");

  zx_handle_t old_hnd = event.get();
  ASSERT_OK(event.replace(0, &event_repl));
  ASSERT_EQ(event.get(), ZX_HANDLE_INVALID);
  ASSERT_NE(old_hnd, event_repl.get());
}

TEST_F(FakeObjectTest, HandleClose) {
  zx::result<zx_handle_t> obj = CreateFakeObject();
  EXPECT_OK(obj.status_value());
  EXPECT_NE(obj.value(), ZX_HANDLE_INVALID);
  EXPECT_EQ(1u, FakeHandleTable().size());

  EXPECT_OK(zx_handle_close(obj.value()));
  EXPECT_EQ(0u, FakeHandleTable().size());
}

TEST(FakeObject, HandleCloseMany) {
  // Ensure other test state was cleaned up.
  ASSERT_EQ(0, FakeHandleTable().size());
  std::array<zx_handle_t, 4> handles = {ZX_HANDLE_INVALID};

  zx::result<zx_handle_t> obj_res = CreateFakeObject();
  EXPECT_OK(obj_res.status_value());
  handles[0] = obj_res.value();
  EXPECT_OK(zx_event_create(0, &handles[1]));
  // [2] will be ZX_HANDLE_INVALID
  EXPECT_OK(zx_event_create(0, &handles[3]));

  ASSERT_NO_DEATH([handles]() { EXPECT_OK(zx_handle_close_many(handles.data(), handles.size())); });
}

TEST_F(FakeObjectTest, WaitMany) {
  std::array<zx_wait_item_t, 3> items;
  EXPECT_OK(zx_event_create(0, &items[0].handle));
  EXPECT_OK(zx_event_create(0, &items[1].handle));
  zx::result<zx_handle_t> obj_res = CreateFakeObject();
  EXPECT_OK(obj_res.status_value());
  items[2].handle = obj_res.value();

  // This should assert due to a fake handle being in the list of wait items.
  ASSERT_DEATH(
      ([&items] { ASSERT_OK(zx_object_wait_many(items.data(), items.size(), ZX_TIME_INFINITE)); }),
      "");

  // This should behave normally due to being real events and simply return the timeout error.
  ASSERT_NO_DEATH(([&items] {
                    ASSERT_EQ(zx_object_wait_many(items.data(), items.size() - 1,
                                                  zx_deadline_after(ZX_MSEC(1))),
                              ZX_ERR_TIMED_OUT);
                  }),
                  "");

  for (auto& item : items) {
    EXPECT_OK(zx_handle_close(item.handle));
  }
}

// Ensure objects are walked in-order when ForEach is called.
TEST_F(FakeObjectTest, ForEach) {
  std::unordered_map<zx_koid_t, bool> fake_objects;
  for (int i = 0; i < 16; i++) {
    zx::result<zx_handle_t> obj_res = CreateFakeObject();
    ASSERT_TRUE(obj_res.is_ok());
    zx::result<zx_koid_t> koid_res = GetFakeObjectKoid(obj_res.value());
    ASSERT_TRUE(koid_res.is_ok());
    fake_objects[koid_res.value()] = false;
  }

  // Walk the objects ensuring that we've seen all the koids we inserted before.
  FakeHandleTable().ForEach(ZX_OBJ_TYPE_NONE, [&fake_objects](FakeObject* obj) -> bool {
    fake_objects[obj->get_koid()] = true;
    return true;
  });

  // Ensure every object was seen in the ForEach.
  for (auto& pair : fake_objects) {
    EXPECT_TRUE(pair.second);
  }

  // Clean up the 16 objects.
  FakeHandleTable().Clear();
}

// Ensure fake objects can be transmitted over a channel.
TEST_F(FakeObjectTest, Channel) {
  zx::channel in, out;
  ASSERT_OK(zx::channel::create(0, &in, &out));
  auto result = CreateFakeObject();
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(FakeHandleTable::IsValidFakeHandle(result.value()));
  ASSERT_OK(in.write(0, nullptr, 0, &result.value(), 1));

  zx::handle handle;
  uint32_t actual_handles = 0;
  ASSERT_OK(out.read(0, nullptr, handle.reset_and_get_address(), 0, 1, nullptr, &actual_handles));
  ASSERT_EQ(actual_handles, 1);
  ASSERT_TRUE(FakeHandleTable::IsValidFakeHandle(handle.get()));
}

// Ensure fake objects can be transmitted over a channel call.
TEST_F(FakeObjectTest, ChannelCall) {
  zx::channel in, out;
  ASSERT_OK(zx::channel::create(0, &in, &out));
  auto result = CreateFakeObject();
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(FakeHandleTable::IsValidFakeHandle(result.value()));
  std::vector<std::byte> message_bytes(4u);
  zx_channel_call_args_t args{
      .wr_bytes = message_bytes.data(),
      .wr_handles = &*result,
      .rd_bytes = message_bytes.data(),
      .rd_handles = nullptr,
      .wr_num_bytes = static_cast<uint32_t>(message_bytes.size()),
      .wr_num_handles = 1u,
      .rd_num_bytes = static_cast<uint32_t>(message_bytes.size()),
      .rd_num_handles = 1u,
  };
  uint32_t actual_bytes = 0u;
  uint32_t actual_handles = 0u;
  auto responder_thread = std::thread([&] {
    std::vector<std::byte> bytes(4u);
    std::vector<zx_handle_t> handles(1u);
    ASSERT_OK(out.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    ASSERT_OK(out.read(0u, bytes.data(), handles.data(), static_cast<uint32_t>(bytes.size()),
                       static_cast<uint32_t>(handles.size()), nullptr, nullptr));
    zx::handle read_handle(handles[0]);
    ASSERT_OK(out.write(0u, bytes.data(), static_cast<uint32_t>(bytes.size()), nullptr, 0u));
  });
  ASSERT_OK(in.call(0u, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            "actual_bytes %u actual_handles %u", actual_bytes, actual_handles);
  responder_thread.join();
  EXPECT_EQ(actual_bytes, 4u);
  EXPECT_EQ(actual_handles, 0u);
}

// Verify that we drop type testing for fake objects which is a requirement for
// working with FIDL bindings.
TEST_F(FakeObjectTest, ChannelEtc) {
  zx::channel in, out;
  ASSERT_OK(zx::channel::create(0, &in, &out));
  zx_obj_type_t test_type = ZX_OBJ_TYPE_BTI;
  auto result = CreateFakeObject(test_type);
  ASSERT_TRUE(result.is_ok());
  zx_handle_t fake_obj = result.value();
  ASSERT_TRUE(FakeHandleTable::IsValidFakeHandle(fake_obj));

  // We need some real objects to toss into the channel to verify we don't break them.
  zx::vmo vmo;
  zx::event event;
  zx::eventpair ep1, ep2;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), /*options=*/0, &vmo));
  ASSERT_OK(zx::event::create(/*options=*/0, &event));
  ASSERT_OK(zx::eventpair::create(/*options=*/0, &ep1, &ep2));

  // The default operation is to move handles over the channel, but this test
  // uses duplication so that we don't invalidate the test handles before the
  // final run where we intend to be fully successful.
  std::array<zx_handle_disposition_t, 5> wr_handles{
      zx_handle_disposition_t{
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = vmo.release(),
          .type = ZX_OBJ_TYPE_VMO,
          .rights = ZX_RIGHT_SAME_RIGHTS,
      },
      {
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = event.release(),
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
      },
      // This is the fake, which will attempt to masquerade as a handle to a BTI.
      {
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = fake_obj,
          .type = test_type,
          .rights = ZX_RIGHT_SAME_RIGHTS,
      },
      // Intentionally set this eventpair to the wrong type so we know type
      // checking still works generally.
      {
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = ep1.release(),
          .type = ZX_OBJ_TYPE_VMO,
          .rights = ZX_RIGHT_SAME_RIGHTS,
      },
      {
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = ep2.release(),
          .type = ZX_OBJ_TYPE_EVENTPAIR,
          .rights = ZX_RIGHT_SAME_RIGHTS,
      }};

  ASSERT_STATUS(in.write_etc(/*flags=*/0, /*bytes=*/nullptr, /*num_bytes=*/0, wr_handles.data(),
                             wr_handles.size()),
                ZX_ERR_WRONG_TYPE);
  // The write_etc should fail due to ep1, but our fake should have not had a
  // new value written to it since it was fine.
  ASSERT_STATUS(wr_handles[2].result, ZX_OK);
  ASSERT_STATUS(wr_handles[3].result, ZX_ERR_WRONG_TYPE);

  // Fix up ep1 and try again.
  wr_handles[3].type = ZX_OBJ_TYPE_EVENTPAIR;
  wr_handles[3].result = ZX_OK;
  // Testing MOVE this time.
  for (auto& handle : wr_handles) {
    handle.operation = ZX_HANDLE_OP_MOVE;
  }
  ASSERT_OK(in.write_etc(/*flags=*/0, /*bytes=*/nullptr, /*num_bytes=*/0, wr_handles.data(),
                         wr_handles.size()));
  EXPECT_OK(wr_handles[0].result);
  EXPECT_OK(wr_handles[1].result);
  EXPECT_OK(wr_handles[2].result);
  EXPECT_OK(wr_handles[3].result);
  EXPECT_OK(wr_handles[4].result);

  // Verify we fix the incoming handle types from VMO to their proper types.
  std::array<zx_handle_info_t, 5> rd_handles;
  uint32_t actual_handles;
  ASSERT_OK(out.read_etc(/*flags=*/0, /*bytes=*/nullptr, rd_handles.data(), /*num_bytes=*/0,
                         /*num_handles=*/wr_handles.size(), /*actual_bytes=*/nullptr,
                         &actual_handles));
  ASSERT_EQ(rd_handles.size(), actual_handles);
  EXPECT_EQ(ZX_OBJ_TYPE_VMO, rd_handles[0].type);
  EXPECT_EQ(ZX_OBJ_TYPE_EVENT, rd_handles[1].type);
  EXPECT_EQ(test_type, rd_handles[2].type);
  EXPECT_EQ(ZX_OBJ_TYPE_EVENTPAIR, rd_handles[3].type);
  EXPECT_EQ(ZX_OBJ_TYPE_EVENTPAIR, rd_handles[4].type);

  for (auto& handle : rd_handles) {
    EXPECT_OK(zx_handle_close(handle.handle));
  }
}

}  // namespace fake_object::test
