// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/zx/pager.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>

#include <zxtest/zxtest.h>

#include "helpers.h"

namespace {

// Tests that a reference can see parent writes and vice versa.
TEST(VmoReference, Write) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Write to the parent.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  // The reference should see the write.
  uint8_t buf;
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);

  // Write to the reference.
  data = 0xbb;
  ASSERT_OK(ref.write(&data, 0, sizeof(data)));

  // The parent should see the write.
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);
}

// Tests that the ZERO_CHILDREN signal is activated on reference creation.
TEST(VmoReference, ZeroChildren) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Currently the parent has no children, so ZX_VMO_ZERO_CHILDREN should be set.
  zx_signals_t pending;
  ASSERT_OK(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite(), &pending));
  ASSERT_EQ(pending & ZX_VMO_ZERO_CHILDREN, ZX_VMO_ZERO_CHILDREN);

  // Create a reference.
  zx::vmo child;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &child));

  // Currently the parent has one child, so ZX_VMO_ZERO_CHILDREN should be
  // cleared.  Since child VMO creation is synchronous, this signal must already
  // be clear.
  ASSERT_EQ(ZX_ERR_TIMED_OUT,
            vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &pending));
  ASSERT_EQ(pending & ZX_VMO_ZERO_CHILDREN, 0);

  // Close the child reference.
  child.reset();

  // Closing the child doesn't strictly guarantee that ZX_VMO_ZERO_CHILDREN is set
  // immediately, but it should be set very soon if not already.
  ASSERT_OK(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite(), &pending));
  ASSERT_EQ(pending & ZX_VMO_ZERO_CHILDREN, ZX_VMO_ZERO_CHILDREN);
}

// Tests SNAPSHOT clone creation against a reference.
TEST(VmoReference, ChildSnapshot) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Create a child of the reference.
  zx::vmo child;
  ASSERT_OK(ref.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));

  zx_info_vmo_t parent_info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr));

  zx_info_vmo_t child_info;
  ASSERT_OK(child.get_info(ZX_INFO_VMO, &child_info, sizeof(child_info), nullptr, nullptr));

  EXPECT_EQ(parent_info.koid, child_info.parent_koid);

  // The child should logically be a snapshot child of the root vmo.
  uint8_t buf;
  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);

  // Write the page in the root again.
  uint8_t new_data = 0xbb;
  ASSERT_OK(vmo.write(&new_data, 0, sizeof(new_data)));

  // The new write should not be visible to the child.
  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);

  // Write the page in the child.
  data = 0xcc;
  ASSERT_OK(child.write(&data, 0, sizeof(data)));

  // The write should not be visible in the root.
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  // Verify contents of both children.
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);
}

// Tests SNAPSHOT_AT_LEAST_ON_WRITE creation against a reference.
TEST(VmoReference, ChildSnapshotAtLeastOnWrite) {
  zx::vmo vmo;

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  zx::vmo aux;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &aux));
  ASSERT_OK(pager.supply_pages(vmo, 0, zx_system_get_page_size(), aux, 0));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Create a child of the reference.
  zx::vmo child;
  ASSERT_OK(ref.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, zx_system_get_page_size(),
                             &child));

  zx_info_vmo_t parent_info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr));

  zx_info_vmo_t child_info;
  ASSERT_OK(child.get_info(ZX_INFO_VMO, &child_info, sizeof(child_info), nullptr, nullptr));

  EXPECT_EQ(parent_info.koid, child_info.parent_koid);

  // The child should logically be a CoW child of the root vmo.
  uint8_t buf;
  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);

  // Write the page in the root again.
  uint8_t new_data = 0xbb;
  ASSERT_OK(vmo.write(&new_data, 0, sizeof(new_data)));

  // The new write should be visible to the child.
  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  // Write the page in the child.
  data = 0xcc;
  ASSERT_OK(child.write(&data, 0, sizeof(data)));

  // The write should not be visible in the root.
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  // Verify contents of both children.
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);
}

// Tests SLICE creation against a reference.
TEST(VmoReference, ChildSlice) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Create a child of the reference.
  zx::vmo child;
  ASSERT_OK(ref.create_child(ZX_VMO_CHILD_SLICE, 0, zx_system_get_page_size(), &child));

  zx_info_vmo_t parent_info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr));

  zx_info_vmo_t child_info;
  ASSERT_OK(child.get_info(ZX_INFO_VMO, &child_info, sizeof(child_info), nullptr, nullptr));

  EXPECT_EQ(parent_info.koid, child_info.parent_koid);

  // The child should logically be a slice of the root vmo.
  uint8_t buf;
  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);

  // Write the page in the parent again.
  uint8_t new_data = 0xbb;
  ASSERT_OK(vmo.write(&new_data, 0, sizeof(new_data)));

  // The new write should be visible to the child.
  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  // Write the page in the child.
  data = 0xcc;
  ASSERT_OK(child.write(&data, 0, sizeof(data)));

  // The write should be visible in the parent.
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);

  // Verify contents of both children.
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);

  ASSERT_OK(child.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);
}

// Tests nested references.
TEST(VmoReference, NestedChild) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Create a reference of the reference.
  zx::vmo child1;
  ASSERT_OK(ref.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &child1));

  zx_info_vmo_t parent_info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr));

  zx_info_vmo_t child_info;
  ASSERT_OK(child1.get_info(ZX_INFO_VMO, &child_info, sizeof(child_info), nullptr, nullptr));

  EXPECT_EQ(parent_info.koid, child_info.parent_koid);

  // Create a reference of the nested reference.
  zx::vmo child2;
  ASSERT_OK(child1.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &child2));
  ASSERT_OK(child1.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr));
  ASSERT_OK(child2.get_info(ZX_INFO_VMO, &child_info, sizeof(child_info), nullptr, nullptr));
  EXPECT_EQ(parent_info.koid, child_info.parent_koid);

  // Both nested children should logically be references of the root vmo.
  uint8_t buf;
  ASSERT_OK(child1.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  ASSERT_OK(child2.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);

  // Write the page in the parent again.
  uint8_t new_data = 0xbb;
  ASSERT_OK(vmo.write(&new_data, 0, sizeof(new_data)));

  // The new write should be visible to the nested children.
  ASSERT_OK(child1.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);
  ASSERT_OK(child2.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, new_data);
  EXPECT_NE(buf, data);

  // Write the page in the last level child.
  data = 0xcc;
  ASSERT_OK(child2.write(&data, 0, sizeof(data)));

  // The write should be visible in the parent.
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);

  // Verify contents of all children.
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);

  ASSERT_OK(child1.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, data);
  EXPECT_NE(buf, new_data);
}

// Tests child creation against a reference that is unsupported for the parent.
TEST(VmoReference, UnsupportedChild) {
  zx::vmo vmo;

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Creating a snapshot clone of the parent should fail.
  zx::vmo child;
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));

  // Creating a snapshot clone of the reference should also fail.
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));
}

// Tests updates to mappings created against a reference.
TEST(VmoReference, MappingUpdate) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Map the reference.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ref, 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Can see initial contents through the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  EXPECT_EQ(data, *buf);

  // Modify the VMO and verify the change is visible through the mapping.
  data = 0xbb;
  ASSERT_OK(ref.write(&data, 0, sizeof(data)));
  EXPECT_EQ(data, *buf);

  // Decommit the page.
  ASSERT_OK(ref.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0));

  // Both the parent and the child should have no pages populated.
  zx_info_vmo_t info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);

  // The mapping should now read zeros.
  EXPECT_EQ(0u, *buf);
}

// Tests attributed pages for references.
TEST(VmoReference, AttributedCounts) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Commit a page in the parent.
  const uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  // The parent should see the page populated while the reference does not.
  zx_info_vmo_t info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(zx_system_get_page_size(), info.populated_bytes);

  // The reference should see the page.
  uint8_t buf;
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);

  // Drop the parent.
  vmo.reset();

  // Committed pages still not attributed to the reference.
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);

  // The reference can read the parent's page though.
  buf = 0;
  ASSERT_OK(ref.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);
}

// Tests resizing a reference and resizing the parent.
TEST(VmoReference, Resize) {
  const uint64_t kInitialSize = 4 * zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kInitialSize, ZX_VMO_RESIZABLE, &vmo));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_RESIZABLE, 0, 0, &ref));

  // Resize the parent.
  const uint64_t kVmoSize = 2 * zx_system_get_page_size();
  const uint64_t kVmoContentSize = 2 * zx_system_get_page_size() - sizeof(uint64_t);
  ASSERT_OK(vmo.set_size(kVmoContentSize));
  uint64_t size;
  ASSERT_OK(vmo.get_size(&size));
  EXPECT_EQ(kVmoSize, size);
  ASSERT_OK(vmo.get_prop_content_size(&size));
  EXPECT_EQ(kVmoContentSize, size);

  // The reference should see the resize.
  ASSERT_OK(ref.get_size(&size));
  EXPECT_EQ(kVmoSize, size);
  ASSERT_OK(ref.get_prop_content_size(&size));
  EXPECT_EQ(kVmoContentSize, size);

  // Resize the reference.
  const uint64_t kRefSize = 3 * zx_system_get_page_size();
  const uint64_t kRefContentSize = 3 * zx_system_get_page_size() - sizeof(uint64_t);
  ASSERT_OK(ref.set_size(kRefContentSize));
  ASSERT_OK(ref.get_size(&size));
  EXPECT_EQ(kRefSize, size);
  ASSERT_OK(ref.get_prop_content_size(&size));
  EXPECT_EQ(kRefContentSize, size);

  // The parent should see the resize.
  ASSERT_OK(vmo.get_size(&size));
  EXPECT_EQ(kRefSize, size);
  ASSERT_OK(vmo.get_prop_content_size(&size));
  EXPECT_EQ(kRefContentSize, size);
}

// Tests resizing using a non-resizable reference handle.
TEST(VmoReference, UnsupportedResize) {
  const uint64_t kInitialSize = 4 * zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kInitialSize, ZX_VMO_RESIZABLE, &vmo));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Cannot resize a non-resizable reference.
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, ref.set_size(zx_system_get_page_size()));

  // The reference should still see the parent's resize.
  const uint64_t kVmoSize = 2 * zx_system_get_page_size();
  const uint64_t kVmoContentSize = 2 * zx_system_get_page_size() - sizeof(uint64_t);
  ASSERT_OK(vmo.set_size(kVmoContentSize));
  uint64_t size;
  ASSERT_OK(vmo.get_size(&size));
  EXPECT_EQ(kVmoSize, size);
  ASSERT_OK(vmo.get_prop_content_size(&size));
  EXPECT_EQ(kVmoContentSize, size);

  // The reference should see the resize.
  ASSERT_OK(ref.get_size(&size));
  EXPECT_EQ(kVmoSize, size);
  ASSERT_OK(ref.get_prop_content_size(&size));
  EXPECT_EQ(kVmoContentSize, size);

  // Cannot create a resizable reference of a non-resizable VMO.
  zx::vmo noresize;
  ASSERT_OK(zx::vmo::create(kInitialSize, 0, &noresize));

  zx::vmo noresizeref;
  ASSERT_EQ(
      ZX_ERR_ACCESS_DENIED,
      noresize.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_RESIZABLE, 0, 0, &noresizeref));

  // Can create a non-resizable reference of a non-resizable VMO.
  ASSERT_OK(noresize.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &noresizeref));

  // Cannot resize a non-resizable reference of a non-resizable VMO.
  ASSERT_EQ(ZX_ERR_UNAVAILABLE, noresize.set_size(zx_system_get_page_size()));
}

// Tests creating streams against references to the same VMO.
TEST(VmoReference, Streams) {
  const uint64_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref1;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref1));

  // Create a stream against a reference.
  zx::stream stream1;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, ref1, 0, &stream1));

  uint8_t buffer[1] = {};
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };

  // Should be able to read previously written VMO contents.
  uint64_t actual = 0;
  ASSERT_OK(stream1.readv(0, &vec, 1, &actual));
  EXPECT_EQ(actual, vec.capacity);
  EXPECT_EQ(data, buffer[0]);

  zx::vmo ref2;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref2));

  // Create a stream against a reference.
  zx::stream stream2;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, ref2, 0, &stream2));

  data = 0xbb;
  buffer[0] = data;

  actual = 0;
  ASSERT_OK(stream2.writev(0, &vec, 1, &actual));
  EXPECT_EQ(actual, vec.capacity);

  // Should have been able to write to the parent VMO.
  uint8_t buf;
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);

  // The references should see the write too.
  ASSERT_OK(ref1.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);

  ASSERT_OK(ref2.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(data, buf);

  // The other stream should see the write too.
  ASSERT_OK(stream1.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, nullptr));
  actual = 0;
  ASSERT_OK(stream1.readv(0, &vec, 1, &actual));
  EXPECT_EQ(actual, vec.capacity);
  EXPECT_EQ(data, buffer[0]);
}

// Tests mapping update after dropping the reference.
TEST(VmoReference, MappingUpdateAfterDroppingRef) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Map the VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Can see initial contents through the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  EXPECT_EQ(data, *buf);

  // Modify the VMO and verify the change is visible through the mapping.
  data = 0xbb;
  ASSERT_OK(ref.write(&data, 0, sizeof(data)));
  EXPECT_EQ(data, *buf);

  // Drop the reference.
  ref.reset();

  // Decommit the page.
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0));

  zx_info_vmo_t info;
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);

  // The mapping should now read zeros.
  EXPECT_EQ(0u, *buf);
}

// Tests mapping update after dropping the parent.
TEST(VmoReference, MappingUpdateAfterDroppingParent) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Map the reference.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ref, 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Can see initial contents through the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  EXPECT_EQ(data, *buf);

  // Modify the VMO and verify the change is visible through the mapping.
  data = 0xbb;
  ASSERT_OK(ref.write(&data, 0, sizeof(data)));
  EXPECT_EQ(data, *buf);

  // Drop the parent.
  vmo.reset();

  // Decommit the page.
  ASSERT_OK(ref.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0));

  zx_info_vmo_t info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);

  // The mapping should now read zeros.
  EXPECT_EQ(0u, *buf);
}

// Tests mapping udpate after dropping a nested reference.
TEST(VmoReference, MappingUpdateNestedChildDestroy) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  // Create a nested reference.
  zx::vmo nested;
  ASSERT_OK(ref.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &nested));

  // Map the nested reference.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, nested, 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Can see initial contents through the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  EXPECT_EQ(data, *buf);

  // Modify the VMO and verify the change is visible through the mapping.
  data = 0xbb;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));
  EXPECT_EQ(data, *buf);

  // Drop the first level reference.
  ref.reset();

  // Decommit the page.
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0));

  zx_info_vmo_t info;
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);

  // The mapping should now read zeros.
  EXPECT_EQ(0u, *buf);
}

// Tests mapping update after dropping the parent in a nested reference hierarchy.
TEST(VmoReference, MappingUpdateNestedRootDestroy) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  // Write a non-zero page.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));

  // Create two references.
  zx::vmo ref1;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref1));
  zx::vmo ref2;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref2));

  // Create a nested reference.
  zx::vmo nested;
  ASSERT_OK(ref1.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &nested));

  // Map the references.
  zx_vaddr_t ptr1;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ref1, 0,
                                       zx_system_get_page_size(), &ptr1));

  auto unmap1 = fit::defer([&]() {
    // Cleanup the mappings we created.
    zx::vmar::root_self()->unmap(ptr1, zx_system_get_page_size());
  });

  zx_vaddr_t ptr2;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ref2, 0,
                                       zx_system_get_page_size(), &ptr2));

  auto unmap2 = fit::defer([&]() {
    // Cleanup the mappings we created.
    zx::vmar::root_self()->unmap(ptr2, zx_system_get_page_size());
  });

  // Map the nested reference.
  zx_vaddr_t ptr3;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, nested, 0,
                                       zx_system_get_page_size(), &ptr3));

  auto unmap3 = fit::defer([&]() {
    // Cleanup the mappings we created.
    zx::vmar::root_self()->unmap(ptr3, zx_system_get_page_size());
  });

  // Can see initial contents through all the mappings.
  auto buf = reinterpret_cast<uint8_t*>(ptr1);
  EXPECT_EQ(data, *buf);
  buf = reinterpret_cast<uint8_t*>(ptr2);
  EXPECT_EQ(data, *buf);
  buf = reinterpret_cast<uint8_t*>(ptr3);
  EXPECT_EQ(data, *buf);

  // Modify the VMO and verify the change is visible through the mappings.
  data = 0xbb;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));
  buf = reinterpret_cast<uint8_t*>(ptr1);
  EXPECT_EQ(data, *buf);
  buf = reinterpret_cast<uint8_t*>(ptr2);
  EXPECT_EQ(data, *buf);
  buf = reinterpret_cast<uint8_t*>(ptr3);
  EXPECT_EQ(data, *buf);

  // Drop the root VMO.
  vmo.reset();

  // Decommit the page.
  ASSERT_OK(nested.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0));

  zx_info_vmo_t info;
  ASSERT_OK(nested.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_bytes);

  // The mappings should now read zeros.
  buf = reinterpret_cast<uint8_t*>(ptr1);
  EXPECT_EQ(0u, *buf);
  buf = reinterpret_cast<uint8_t*>(ptr2);
  EXPECT_EQ(0u, *buf);
  buf = reinterpret_cast<uint8_t*>(ptr3);
  EXPECT_EQ(0u, *buf);
}

// Tests that a reference is not reported as a CoW child.
TEST(VmoReference, GetInfo) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::vmo ref;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_REFERENCE, 0, 0, &ref));

  zx_info_vmo_t parent_info;
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr));

  zx_info_vmo_t ref_info;
  ASSERT_OK(ref.get_info(ZX_INFO_VMO, &ref_info, sizeof(ref_info), nullptr, nullptr));

  EXPECT_EQ(parent_info.koid, ref_info.parent_koid);
  EXPECT_EQ(0u, ref_info.flags & ZX_INFO_VMO_IS_COW_CLONE);
}

}  // namespace
