// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/iob/blob-id-allocator.h>
#include <lib/zx/channel.h>
#include <lib/zx/iob.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/process.h>
#include <zircon/rights.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iob.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/port.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <cstring>
#include <thread>
#include <unordered_set>

#include <zxtest/zxtest.h>

#include "../needs-next.h"

const uint64_t kIoBufferEpRwMap = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE |
                                  ZX_IOB_ACCESS_EP1_CAN_MAP_READ | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE;
const uint64_t kIoBufferEp0OnlyRwMap =
    ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE;
const uint64_t kIoBufferRdOnlyMap = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP1_CAN_MAP_READ;

NEEDS_NEXT_SYSCALL(zx_iob_allocate_id);
NEEDS_NEXT_SYSCALL(zx_iob_create_shared_region);
NEEDS_NEXT_SYSCALL(zx_iob_writev);

namespace {
// An RAII Helper used to make sure that we don't accidentally leak any mapped
// IOBs during testing.
class MappingHelper {
 public:
  ~MappingHelper() { Unmap(); }

  MappingHelper(const MappingHelper&) = delete;
  MappingHelper& operator=(const MappingHelper&) = delete;

  MappingHelper& operator=(MappingHelper&& other) {
    this->Unmap();
    std::swap(addr_, other.addr_);
    std::swap(region_len_, other.region_len_);
    return *this;
  }
  MappingHelper(MappingHelper&& other) { *this = std::move(other); }

  static zx::result<MappingHelper> Create(zx_vm_option_t options, size_t vmar_offset,
                                          const zx::iob& iob_handle, uint32_t region_index,
                                          uint64_t region_offset, size_t region_len) {
    zx_vaddr_t addr{0};
    zx_status_t res = zx::vmar::root_self()->map_iob(options, vmar_offset, iob_handle, region_index,
                                                     region_offset, region_len, &addr);
    if (res != ZX_OK) {
      return zx::error(res);
    }

    return zx::ok(MappingHelper{addr, region_len});
  }

  zx_status_t Unmap() {
    if (addr_ != 0) {
      zx_status_t res = zx::vmar::root_self()->unmap(addr_, region_len_);
      addr_ = 0;
      region_len_ = 0;
      return res;
    }
    return ZX_ERR_BAD_STATE;
  }

  zx_vaddr_t addr() const { return addr_; }
  size_t region_len() const { return region_len_; }

 private:
  MappingHelper(zx_vaddr_t addr, size_t region_len) : addr_(addr), region_len_(region_len) {}

  zx_vaddr_t addr_{0};
  size_t region_len_{0};
};

TEST(Iob, Create) {
  const uint64_t page_size = zx_system_get_page_size();

  zx_handle_t ep0, ep1;
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  };
  EXPECT_OK(zx_iob_create(0, &config, 1, &ep0, &ep1));
  EXPECT_OK(zx_handle_close(ep0));
  EXPECT_OK(zx_handle_close(ep1));

  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, nullptr, 0, &ep0, &ep1));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(ep0));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(ep1));

  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, nullptr, 4, &ep0, &ep1));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(ep0));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_handle_close(ep1));

  zx_iob_region_t no_access_config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = 0,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &no_access_config, 1, &ep0, &ep1));
}

TEST(Iob, CreateHuge) {
  zx::iob ep0, ep1;
  // Iobs will round up to the nearest page size. Make sure we don't overflow and wrap around.
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = 0xFFFF'FFFF'FFFF'FFFF,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  };
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, zx::iob::create(0, &config, 1, &ep0, &ep1));
}

TEST(Iob, BadVmOptions) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0xFFF,
          },
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx::iob::create(0, &config, 1, &ep0, &ep1));
}

TEST(Iob, PeerClosed) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  };
  EXPECT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));
  zx_signals_t observed;
  EXPECT_EQ(ZX_ERR_TIMED_OUT,
            ep0.wait_one(ZX_IOB_PEER_CLOSED, zx::time::infinite_past(), &observed));
  EXPECT_EQ(0, observed);
  EXPECT_EQ(ZX_ERR_TIMED_OUT,
            ep1.wait_one(ZX_IOB_PEER_CLOSED, zx::time::infinite_past(), &observed));
  EXPECT_EQ(0, observed);

  ep0.reset();
  EXPECT_OK(ep1.wait_one(ZX_IOB_PEER_CLOSED, zx::time{0}, &observed));
  EXPECT_EQ(ZX_IOB_PEER_CLOSED, observed);
  ep1.reset();

  EXPECT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));
  ep1.reset();
  EXPECT_OK(ep0.wait_one(ZX_IOB_PEER_CLOSED, zx::time{0}, &observed));
  EXPECT_EQ(ZX_IOB_PEER_CLOSED, observed);
  ep0.reset();
}

TEST(Iob, RegionMap) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[1]{{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  }};

  ASSERT_OK(zx::iob::create(0, config, 1, &ep0, &ep1));

  zx::result<MappingHelper> region1 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(region1.status_value());
  ASSERT_NE(0, region1->addr());

  // If we write data to the mapped memory of one handle, we should be able to read it from the
  // mapped memory of the other handle
  const char* test_str = "ABCDEFG";
  char* data1 = reinterpret_cast<char*>(region1->addr());
  memcpy(data1, test_str, 1 + strlen(test_str));
  EXPECT_STREQ(data1, test_str);

  zx::result<MappingHelper> region2 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep1, 0, 0, page_size);
  ASSERT_OK(region2.status_value());
  char* data2 = reinterpret_cast<char*>(region2->addr());
  EXPECT_STREQ(data2, test_str);
}

TEST(Iob, MappingRights) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  constexpr size_t noPermissionsIdx = 0;
  constexpr size_t onlyEp0Idx = 1;
  constexpr size_t rdOnlyIdx = 2;
  // There are 4 factors that go into the resulting permissions of a mapped region:
  // - The options the region it was created with
  // - The handle rights of the iob we have
  // - The handle rights of the vmar we have
  // - The VmOptions that the map call requests
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferRdOnlyMap | ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
          .private_region =
              {
                  .options = 0,
              },
      }};

  // Let's create some regions with varying r/w/map options
  ASSERT_OK(zx::iob::create(0, config, 3, &ep0, &ep1));

  // If the iorb handle doesn't have the correct rights, we shouldn't be able to map it
  zx::iob no_write_ep_handle;
  zx::iob no_read_write_ep_handle;
  zx::iob no_map_ep_handle;
  zx::unowned_vmar vmar = zx::vmar::root_self();

  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_WRITE, &no_write_ep_handle));
  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_WRITE & ~ZX_RIGHT_READ,
                          &no_read_write_ep_handle));
  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_MAP, &no_map_ep_handle));

  // We shouldn't be able to map a region that didn't set map permissions.
  zx::result<MappingHelper> map1 = MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0,
                                                         noPermissionsIdx, 0, page_size);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, map1.status_value());

  zx::result<MappingHelper> map2 = MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep1,
                                                         noPermissionsIdx, 0, page_size);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, map2.status_value());

  // And if a region is set to only be mappable by one endpoint, ensure it is.
  zx::result<MappingHelper> map3 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep1, onlyEp0Idx, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, map3.status_value());

  zx::result<MappingHelper> map4 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, onlyEp0Idx, 0, page_size);
  EXPECT_EQ(ZX_OK, map4.status_value());

  // If the handle lacks ZX_RIGHT_MAP, mapping should fail even if the region allows it.
  zx::result<MappingHelper> map_no_map = MappingHelper::Create(
      ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, no_map_ep_handle, onlyEp0Idx, 0, page_size);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, map_no_map.status_value());

  // We shouldn't be able to request more rights than the region has
  zx::result<MappingHelper> map5 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, rdOnlyIdx, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, map5.status_value());

  zx::result<MappingHelper> map6 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep1, rdOnlyIdx, 0, 0);
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, map6.status_value());
}

TEST(Iob, PeerClosedMappedReferences) {
  const uint64_t page_size = zx_system_get_page_size();

  // We shouldn't see peer closed until mappings created by an endpoint are also closed.
  zx::iob ep0, ep1;
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  };
  EXPECT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));
  zx::unowned_vmar vmar = zx::vmar::root_self();

  zx::result<MappingHelper> region1 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(region1.status_value());
  ASSERT_NE(0, region1->addr());

  zx::result<MappingHelper> region2 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(region2.status_value());
  ASSERT_NE(0, region2->addr());

  ep0.reset();
  // We shouldn't get peer closed on ep1 just yet.
  zx_signals_t observed;
  EXPECT_EQ(ZX_ERR_TIMED_OUT, ep1.wait_one(ZX_IOB_PEER_CLOSED, zx::time{0}, &observed));
  EXPECT_EQ(0, observed);

  EXPECT_OK(region1->Unmap());
  EXPECT_EQ(ZX_ERR_TIMED_OUT, ep1.wait_one(ZX_IOB_PEER_CLOSED, zx::time{0}, &observed));
  EXPECT_EQ(0, observed);
  EXPECT_OK(region2->Unmap());

  // But eventually we should.
  //
  // Note, we will wait forever for two reasons.  First, unmapping the last
  // region is not guaranteed to synchronously trigger PEER_CLOSED.  Second,
  // this test might be running in a componennt context where some other process
  // might be issuing, say, a zx_object_get_info with ZX_INFO_PROCESS_MAPS call
  // against *this* process, which could result in an extra temporary refcount
  // of the underlying VmObject, thereby delaying the VmObject destruction and
  // transmission of the PEER_CLOSED signal.  So we wait.
  zx_status_t status;
  while (true) {
    zx::time deadline = zx::time(zx_deadline_after(ZX_SEC(5)));
    status = ep1.wait_one(ZX_IOB_PEER_CLOSED, deadline, &observed);
    if (status != ZX_ERR_TIMED_OUT) {
      break;
    }
    printf("waiting for ZX_IOB_PEER_CLOSED...\n");
  }
  EXPECT_OK(status);
  EXPECT_EQ(ZX_IOB_PEER_CLOSED, observed);
  ep1.reset();
}

TEST(Iob, GetInfoIob) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 2 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 3 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      }};

  ASSERT_OK(zx::iob::create(0, config, 3, &ep0, &ep1));

  zx_iob_region_info_t info[5];
  size_t actual;
  size_t available;
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, &info, sizeof(info), &actual, &available));
  EXPECT_EQ(actual, 3);
  EXPECT_EQ(available, 3);
  EXPECT_BYTES_EQ(&(info[0].region), &config[0], sizeof(zx_iob_region_t));
  EXPECT_BYTES_EQ(&(info[1].region), &config[1], sizeof(zx_iob_region_t));
  EXPECT_BYTES_EQ(&(info[2].region), &config[2], sizeof(zx_iob_region_t));

  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, nullptr, 0, &actual, &available));
  EXPECT_EQ(actual, 0);
  EXPECT_EQ(available, 3);

  zx_iob_region_info_t info2[2];
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, &info2, sizeof(info2), &actual, &available));
  EXPECT_EQ(actual, 2);
  EXPECT_EQ(available, 3);
  EXPECT_BYTES_EQ(&(info[0].region), &config[0], sizeof(zx_iob_region_t));
  EXPECT_BYTES_EQ(&(info[1].region), &config[1], sizeof(zx_iob_region_t));
}

TEST(Iob, RegionInfoSwappedAccess) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
  };

  ASSERT_OK(zx::iob::create(0, config, 1, &ep0, &ep1));

  zx_iob_region_info_t ep0_info[1];
  zx_iob_region_info_t ep1_info[1];
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, ep0_info, sizeof(ep0_info), nullptr, nullptr));
  ASSERT_OK(ep1.get_info(ZX_INFO_IOB_REGIONS, ep1_info, sizeof(ep1_info), nullptr, nullptr));

  // We should see the same underlying memory object
  EXPECT_EQ(ep0_info[0].koid, ep1_info[0].koid);

  // But our view of the access bits should be swapped
  EXPECT_EQ(ep0_info[0].region.access,
            ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE);
  // ep1 will see itself as ep0, and the other endpoint as ep1
  EXPECT_EQ(ep1_info[0].region.access,
            ZX_IOB_ACCESS_EP1_CAN_MAP_READ | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE);
}

TEST(Iob, GetInfoIobRegions) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 2 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 3 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      }};

  ASSERT_OK(zx::iob::create(0, config, 3, &ep0, &ep1));

  zx_info_iob info;
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.options, 0);
  EXPECT_EQ(info.region_count, 3);
  ASSERT_OK(ep1.get_info(ZX_INFO_IOB, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.options, 0);
  EXPECT_EQ(info.region_count, 3);
}

TEST(Iob, RoundedSizes) {
  const uint64_t page_size = zx_system_get_page_size();

  // Check that iobs round up their requested size to the nearest page
  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size + 1,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size - 1,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      }};

  ASSERT_OK(zx::iob::create(0, config, 3, &ep0, &ep1));

  zx_iob_region_info_t info[3];
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info[0].region.size, page_size);
  EXPECT_EQ(info[1].region.size, 2 * page_size);
  EXPECT_EQ(info[2].region.size, page_size);
}

TEST(Iob, GetSetNames) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
  };
  ASSERT_OK(zx::iob::create(0, config, 1, &ep0, &ep1));

  // If we set the name from ep0,  we should see it from ep1
  const char* iob_name = "TestIob";
  EXPECT_OK(ep0.set_property(ZX_PROP_NAME, iob_name, 8));

  char name_buffer[ZX_MAX_NAME_LEN];
  EXPECT_OK(ep0.get_property(ZX_PROP_NAME, name_buffer, ZX_MAX_NAME_LEN));
  EXPECT_STREQ(name_buffer, iob_name);
  EXPECT_OK(ep1.get_property(ZX_PROP_NAME, name_buffer, ZX_MAX_NAME_LEN));
  EXPECT_STREQ(name_buffer, iob_name);

  const char* iob_name2 = "TestIob2";
  EXPECT_OK(ep1.set_property(ZX_PROP_NAME, iob_name2, 9));
  EXPECT_OK(ep0.get_property(ZX_PROP_NAME, name_buffer, ZX_MAX_NAME_LEN));
  EXPECT_STREQ(name_buffer, iob_name2);
  EXPECT_OK(ep1.get_property(ZX_PROP_NAME, name_buffer, ZX_MAX_NAME_LEN));
  EXPECT_STREQ(name_buffer, iob_name2);

  // The Underlying vmos should also have their name set
  size_t avail;
  ASSERT_OK(
      zx_object_get_info(zx_process_self(), ZX_INFO_PROCESS_VMOS, nullptr, 0, nullptr, &avail));

  auto vmo_infos = std::make_unique<zx_info_vmo_t[]>(avail);
  ASSERT_OK(zx_object_get_info(zx_process_self(), ZX_INFO_PROCESS_VMOS, vmo_infos.get(),
                               sizeof(zx_info_vmo_t) * avail, nullptr, nullptr));

  bool found_vmo = false;
  for (size_t i = 0; i < avail; ++i) {
    zx_info_vmo_t& vmo_info = vmo_infos[i];
    if (0 == strcmp("TestIob2", vmo_info.name)) {
      EXPECT_TRUE(vmo_info.flags & ZX_INFO_VMO_VIA_IOB_HANDLE);
      found_vmo = true;
      break;
    }
  }
  EXPECT_TRUE(found_vmo);
}

/// Iob regions count towards a process's vmo allocations.
TEST(Iob, GetInfoProcessVmos) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap,
          .size = 2 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap,
          .size = 3 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region =
              {
                  .options = 0,
              },
      }};

  // Create the IOB and set the name we will use to identify the VMOs it creates.
  ASSERT_OK(zx::iob::create(0, config, 3, &ep0, &ep1));
  ep0.set_property(ZX_PROP_NAME, "TestIob", 8);

  // Introduce a helper lambda used to find the VMOs which should have been created when we created
  // our IOB.  We will use this to verify all of the expected VMOs were created, and that the proper
  // ones go away when we close each endpoint.
  bool saw_ep0_1_page, saw_ep0_2_page, saw_ep0_3_page;
  bool saw_ep1_1_page, saw_ep1_2_page, saw_ep1_3_page;
  auto FindTestVmos = [&]() -> void {
    saw_ep0_1_page = false;
    saw_ep0_2_page = false;
    saw_ep0_3_page = false;
    saw_ep1_1_page = false;
    saw_ep1_2_page = false;
    saw_ep1_3_page = false;

    size_t vmo_count{0};
    std::unique_ptr<zx_info_vmo_t[]> vmo_infos;
    ASSERT_OK(zx_object_get_info(zx_process_self(), ZX_INFO_PROCESS_VMOS, nullptr, 0, nullptr,
                                 &vmo_count));
    if (vmo_count) {
      vmo_infos = std::make_unique<zx_info_vmo_t[]>(vmo_count);
      ASSERT_OK(zx_object_get_info(zx_process_self(), ZX_INFO_PROCESS_VMOS, vmo_infos.get(),
                                   sizeof(zx_info_vmo_t) * vmo_count, nullptr, nullptr));

      for (size_t i = 0; i < vmo_count; ++i) {
        zx_info_vmo_t& vmo_info = vmo_infos[i];
        if (0 == strcmp("TestIob", vmo_info.name)) {
          EXPECT_EQ(0u, vmo_info.size_bytes % page_size);
          switch (vmo_info.size_bytes / page_size) {
            case 1:
              if ((vmo_info.handle_rights & ZX_RIGHT_READ) == 0) {
                // This is ep1 and we shouldn't have WRITE rights either
                EXPECT_FALSE(vmo_info.handle_rights & ZX_RIGHT_WRITE);
                EXPECT_FALSE(saw_ep1_1_page);
                saw_ep1_1_page = true;
              } else {
                // This is ep0 and we should have write rights
                EXPECT_EQ(ZX_RIGHT_WRITE, vmo_info.handle_rights & ZX_RIGHT_WRITE);
                EXPECT_FALSE(saw_ep0_1_page);
                saw_ep0_1_page = true;
              }
              break;
            case 2:
              if ((vmo_info.handle_rights & ZX_RIGHT_READ) == 0) {
                EXPECT_FALSE(vmo_info.handle_rights & ZX_RIGHT_WRITE);
                EXPECT_FALSE(saw_ep1_2_page);
                saw_ep1_2_page = true;
              } else {
                EXPECT_EQ(ZX_RIGHT_WRITE, vmo_info.handle_rights & ZX_RIGHT_WRITE);
                EXPECT_FALSE(saw_ep0_2_page);
                saw_ep0_2_page = true;
              }
              break;
            case 3:
              if ((vmo_info.handle_rights & ZX_RIGHT_READ) == 0) {
                EXPECT_FALSE(vmo_info.handle_rights & ZX_RIGHT_WRITE);
                EXPECT_FALSE(saw_ep1_3_page);
                saw_ep1_3_page = true;
              } else {
                EXPECT_EQ(ZX_RIGHT_WRITE, vmo_info.handle_rights & ZX_RIGHT_WRITE);
                EXPECT_FALSE(saw_ep0_3_page);
                saw_ep0_3_page = true;
              }
              break;
          }
        }
      }
    }
  };

  // Enumerate all of the VMOs in this process and find our test VMOs.  We have not closed any
  // endpoints yet, should be able to find all of them.
  FindTestVmos();
  EXPECT_TRUE(saw_ep0_1_page);
  EXPECT_TRUE(saw_ep0_2_page);
  EXPECT_TRUE(saw_ep0_3_page);
  EXPECT_TRUE(saw_ep1_1_page);
  EXPECT_TRUE(saw_ep1_2_page);
  EXPECT_TRUE(saw_ep1_3_page);

  // Now close Ep0.  We have not mapped any of these VMOs, so we expect all of the Ep0 VMOs to
  // disappear, while the Ep1 VMOs remain.
  ep0.reset();
  FindTestVmos();
  EXPECT_FALSE(saw_ep0_1_page);
  EXPECT_FALSE(saw_ep0_2_page);
  EXPECT_FALSE(saw_ep0_3_page);
  EXPECT_TRUE(saw_ep1_1_page);
  EXPECT_TRUE(saw_ep1_2_page);
  EXPECT_TRUE(saw_ep1_3_page);

  // Now close Ep1 as well.  All of our test VMOs should be gone after this.
  ep1.reset();
  FindTestVmos();
  EXPECT_FALSE(saw_ep0_1_page);
  EXPECT_FALSE(saw_ep0_2_page);
  EXPECT_FALSE(saw_ep0_3_page);
  EXPECT_FALSE(saw_ep1_1_page);
  EXPECT_FALSE(saw_ep1_2_page);
  EXPECT_FALSE(saw_ep1_3_page);
}

TEST(Iob, GetInfoProcessMaps) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config[3]{{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  }};

  ASSERT_OK(zx::iob::create(0, config, 1, &ep0, &ep1));
  zx::unowned_vmar vmar = zx::vmar::root_self();
  size_t num_mappings_before;
  ASSERT_OK(zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, nullptr, 0, nullptr,
                                          &num_mappings_before));
  zx::result<MappingHelper> region =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(region.status_value());
  ASSERT_NE(0, region->addr());

  size_t num_mappings_after;
  ASSERT_OK(zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, nullptr, 0, nullptr,
                                          &num_mappings_after));

  EXPECT_EQ(num_mappings_after, num_mappings_before + 1);

  // Allocate an array with double the capacity of mappings we saw. This should ensure that if our
  // allocation itself triggers a mapping or two, there should still be plenty of space available.
  auto map_infos = std::make_unique<zx_info_maps_t[]>(num_mappings_after * 2);
  size_t num_map_infos = 0;
  ASSERT_OK(zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, map_infos.get(),
                                          sizeof(zx_info_vmo_t) * num_mappings_after * 2, nullptr,
                                          &num_map_infos));
  // Validate that we were able to fit all the mappings to ensure we
  //   1. cannot have missed our target IOB mapping
  //   2. the loop below will not do an array overrun.
  ASSERT_LE(num_map_infos, num_mappings_after * 2);

  zx_iob_region_info_t ep0_info[1];
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, ep0_info, sizeof(ep0_info), nullptr, nullptr));

  zx_koid_t iob_koid = ep0_info[0].koid;
  bool saw_iob_mapping = false;
  for (size_t i = 0; i < num_map_infos; ++i) {
    zx_info_maps_t& mapping = map_infos[i];
    if (mapping.u.mapping.vmo_koid == iob_koid) {
      EXPECT_EQ(mapping.size, page_size);
      saw_iob_mapping = true;
      break;
    }
  }
  EXPECT_TRUE(saw_iob_mapping);
}

TEST(Iob, IdAllocatorMediatedAccess) {
  NEEDS_NEXT_SKIP(zx_iob_allocate_id);

  constexpr std::array<std::byte, 10> kBlob{std::byte{'a'}};
  constexpr zx_iob_allocate_id_options_t kOptions = 0;
  constexpr uint32_t kIdAllocatorIdx = 0;
  const uint64_t page_size = zx_system_get_page_size();

  // Endpoint 0 will be mapped, while endpoint 1 will facilitate mediated
  // allocations.
  zx::iob ep0, ep1;
  zx_iob_region_t config[]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
          .private_region = {.options = 0},
      },
  };

  ASSERT_OK(zx::iob::create(0, config, std::size(config), &ep0, &ep1));

  zx::result<MappingHelper> mapped_region =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  EXPECT_OK(mapped_region.status_value());
  ASSERT_NE(mapped_region->addr(), 0u);
  cpp20::span<std::byte> bytes{reinterpret_cast<std::byte*>(mapped_region->addr()), page_size};

  // Since mediated access is enabled, the region should already be initialized.
  iob::BlobIdAllocator allocator(bytes);

  // Check that mediated allocation works.
  for (uint64_t expected_id = 0; expected_id < 100; ++expected_id) {
    EXPECT_EQ(expected_id, allocator.next_id());

    uint32_t id;
    if (expected_id % 2 == 0) {
      auto result = allocator.Allocate(kBlob);
      ASSERT_TRUE(result.is_ok());
      id = result.value();
    } else {
      ASSERT_OK(zx_iob_allocate_id(ep1.get(), kOptions, kIdAllocatorIdx, kBlob.data(), kBlob.size(),
                                   &id));
    }
    EXPECT_EQ(expected_id, id);
  }
  EXPECT_EQ(100, allocator.next_id());
}

TEST(Iob, IdAllocatorMediatedErrors) {
  NEEDS_NEXT_SKIP(zx_iob_allocate_id);

  constexpr std::array<std::byte, 10> kBlob{std::byte{'a'}};
  constexpr zx_iob_allocate_id_options_t kOptions = 0;
  constexpr uint32_t kIdAllocatorIdx = 0;
  const uint64_t page_size = zx_system_get_page_size();

  // Endpoint 0 will be mapped, while endpoint 1 will facilitate mediated
  // allocations.
  zx::iob ep0, ep1;
  zx_iob_region_t config[]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
          .private_region = {.options = 0},
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
  };

  ASSERT_OK(zx::iob::create(0, config, std::size(config), &ep0, &ep1));

  zx::result<MappingHelper> region =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(region.status_value());
  ASSERT_NE(region->addr(), 0u);
  cpp20::span<std::byte> bytes{reinterpret_cast<std::byte*>(region->addr()), page_size};

  // Since mediated access is enabled, the region should already be initialized.
  iob::BlobIdAllocator allocator(bytes);

  // ZX_ERR_OUT_OF_RANGE (region index too large)
  {
    uint32_t id;
    zx_status_t status =
        zx_iob_allocate_id(ep1.get(), kOptions, 2, kBlob.data(), kBlob.size(), &id);
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, status);
  }

  // ZX_ERR_WRONG_TYPE (region not of ID_ALLOCATOR discipline)
  {
    uint32_t id;
    zx_status_t status =
        zx_iob_allocate_id(ep1.get(), kOptions, 1, kBlob.data(), kBlob.size(), &id);
    EXPECT_EQ(ZX_ERR_WRONG_TYPE, status);
  }

  // ZX_ERR_INVALID_ARGS (invalid options)
  {
    uint32_t id;
    zx_status_t status =
        zx_iob_allocate_id(ep1.get(), -1, kIdAllocatorIdx, kBlob.data(), kBlob.size(), &id);
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
  }

  // ZX_ERR_ACCESS_DENIED (endpoint not mediated-writable)
  {
    uint32_t id;
    zx_status_t status =
        zx_iob_allocate_id(ep0.get(), kOptions, kIdAllocatorIdx, kBlob.data(), kBlob.size(), &id);
    EXPECT_EQ(ZX_ERR_ACCESS_DENIED, status);
  }

  // ZX_ERR_NO_MEMORY (no memory left in container)
  {
    using AllocateError = iob::BlobIdAllocator::AllocateError;

    fit::result<AllocateError, uint32_t> result = fit::ok(0);
    while (result.is_ok()) {
      result = allocator.Allocate(kBlob);
    }
    ASSERT_EQ(AllocateError::kOutOfMemory, result.error_value());

    uint32_t id;
    zx_status_t status =
        zx_iob_allocate_id(ep1.get(), kOptions, kIdAllocatorIdx, kBlob.data(), kBlob.size(), &id);
    EXPECT_EQ(ZX_ERR_NO_MEMORY, status);
  }

  // ZX_ERR_IO_DATA_INTEGRITY (corrupted memory)
  {
    struct Header {
      uint32_t next_id;
      uint32_t blob_head;
    };
    Header* header = reinterpret_cast<Header*>(bytes.data());
    header->next_id = -1;

    uint32_t id;
    zx_status_t status =
        zx_iob_allocate_id(ep1.get(), kOptions, kIdAllocatorIdx, kBlob.data(), kBlob.size(), &id);
    EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY, status);
  }

  // The whole region should be pinned, so decommitting should result in
  // ZX_ERR_BAD_STATE.
  EXPECT_EQ(ZX_ERR_BAD_STATE, zx::vmar::root_self()->op_range(
                                  ZX_VMAR_OP_DECOMMIT, reinterpret_cast<zx_vaddr_t>(bytes.data()),
                                  bytes.size(), nullptr, 0));
}

TEST(Iob, MapWhenPeerClosed) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region =
          {
              .options = 0,
          },
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));
  ep1.reset();

  zx::result<MappingHelper> region =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  EXPECT_OK(region.status_value());
}

TEST(IobSharedRegion, InvalidArgs) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx_handle_t handle;

  // Non-zero options.
  EXPECT_STATUS(zx_iob_create_shared_region(1, page_size, &handle), ZX_ERR_INVALID_ARGS);

  // Zero size.
  EXPECT_STATUS(zx_iob_create_shared_region(0, 0, &handle), ZX_ERR_INVALID_ARGS);

  // Not multiple of page size
  EXPECT_STATUS(zx_iob_create_shared_region(0, page_size + 3, &handle), ZX_ERR_INVALID_ARGS);
}

TEST(IobSharedRegion, OutOfRange) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  zx::handle handle;

  // Near integer limit
  const uint64_t large_size = std::numeric_limits<uint64_t>::max() - zx_system_get_page_size() + 1;
  ASSERT_EQ(0, large_size % zx_system_get_page_size());
  EXPECT_STATUS(zx_iob_create_shared_region(0, large_size, handle.reset_and_get_address()),
                ZX_ERR_OUT_OF_RANGE);

  // Larger than the maximum VMO size
  uint64_t max_vmo_size;
  {
    zx::vmo unbounded_vmo_for_size_check;
    ASSERT_OK(zx::vmo::create(0, ZX_VMO_UNBOUNDED, &unbounded_vmo_for_size_check));
    ASSERT_OK(unbounded_vmo_for_size_check.get_size(&max_vmo_size));
  }
  EXPECT_STATUS(zx_iob_create_shared_region(0, max_vmo_size + zx_system_get_page_size(),
                                            handle.reset_and_get_address()),
                ZX_ERR_OUT_OF_RANGE);
}

TEST(IobSharedRegion, CreateSucceeds) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx_handle_t handle;
  EXPECT_OK(zx_iob_create_shared_region(0, page_size, &handle));
  zx_handle_close(handle);
}

TEST(Iob, SharedRegionWithNonRingBufferDiscipline) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  for (zx_iob_discipline_type_t type :
       {ZX_IOB_DISCIPLINE_TYPE_NONE, ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR}) {
    zx_iob_region_t config = {
        .type = ZX_IOB_REGION_TYPE_SHARED,
        .access = kIoBufferEpRwMap,
        .discipline = zx_iob_discipline_t{.type = type},
    };
    reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
        .shared_region = shared_region,
    };
    zx::iob ep0, ep1;
    EXPECT_STATUS(zx::iob::create(0, &config, 1, &ep0, &ep1), ZX_ERR_INVALID_ARGS, "type=%lu",
                  type);
  }
}

TEST(Iob, CreateWithMediatedWriteRingBufferDisciplineInvalidArgs) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  // With private region.
  {
    zx_iob_region_t config = {
        .type = ZX_IOB_REGION_TYPE_PRIVATE,
        .access = kIoBufferEpRwMap,
        .size = page_size,
        .discipline =
            zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
    };
    EXPECT_STATUS(zx::iob::create(0, &config, 1, &ep0, &ep1), ZX_ERR_INVALID_ARGS);
  }

  // Region too small: it must be at least two pages.
  {
    zx_handle_t shared_region;
    ASSERT_OK(zx_iob_create_shared_region(0, page_size, &shared_region));
    [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

    zx_iob_region_t config = {
        .type = ZX_IOB_REGION_TYPE_SHARED,
        .access = kIoBufferEpRwMap,
        .discipline =
            zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
    };
    reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
        .shared_region = shared_region,
    };
    EXPECT_STATUS(zx::iob::create(0, &config, 1, &ep0, &ep1), ZX_ERR_INVALID_ARGS);
  }

  // Non-zero size.
  {
    zx_handle_t shared_region;
    ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
    [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

    zx_iob_region_t config = {
        .type = ZX_IOB_REGION_TYPE_SHARED,
        .access = kIoBufferEpRwMap,
        .size = 2 * page_size,  // <- This should be zero
        .discipline =
            zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
    };
    reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
        .shared_region = shared_region,
    };
    EXPECT_STATUS(zx::iob::create(0, &config, 1, &ep0, &ep1), ZX_ERR_INVALID_ARGS);
  }
}

TEST(Iob, CreateWithMediatedWriteRingBufferDisciplineSucceeds) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  EXPECT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));
}

TEST(Iob, IobWriteChecksRights) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // First, remove the write right from the handle.
  zx::iob handle_with_no_rights;
  ep0.duplicate(/*rights=*/0, &handle_with_no_rights);

  char buffer[] = "hello";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };

  EXPECT_STATUS(zx_iob_writev(handle_with_no_rights.get(), 0, 0, &vec, 1), ZX_ERR_ACCESS_DENIED);

  // Endpoint 1 doesn't have mediated write access.
  EXPECT_STATUS(zx_iob_writev(ep1.get(), 0, 0, &vec, 1), ZX_ERR_ACCESS_DENIED);

  // Endpoint 0 does though.
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));
}

TEST(Iob, IobWriteInvalidArgs) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config[] = {
      {
          .type = ZX_IOB_REGION_TYPE_SHARED,
          .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
          .discipline =
              zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      },
  };

  reinterpret_cast<zx_iob_region_shared_t&>(config[0].max_extension) = {
      .shared_region = shared_region,
  };
  ASSERT_OK(zx::iob::create(0, config, std::size(config), &ep0, &ep1));

  char buffer[] = "hello";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };

  // Invalid options.
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 1, 0, &vec, 1), ZX_ERR_INVALID_ARGS);

  // Invalid handle.
  EXPECT_STATUS(zx_iob_writev(ZX_HANDLE_INVALID, 0, 0, &vec, 1), ZX_ERR_BAD_HANDLE);

  // Out of range region.
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 2, &vec, 1), ZX_ERR_OUT_OF_RANGE);

  // Wrong region type.
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 1, &vec, 1), ZX_ERR_WRONG_TYPE);

  // A bad iovec.
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, reinterpret_cast<zx_iovec_t*>(1), 1),
                ZX_ERR_INVALID_ARGS);

  // iovec capacity overflow.
  zx_iovec_t vecs[] = {{
                           .buffer = buffer,
                           .capacity = std::numeric_limits<size_t>::max(),
                       },
                       {
                           .buffer = buffer,
                           .capacity = std::numeric_limits<size_t>::max(),
                       }};
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, vecs, std::size(vecs)), ZX_ERR_INVALID_ARGS);

  // Exceeds max message size.
  constexpr size_t kMaxMessageSize = 65535 - 16;
  auto large_buf = std::make_unique<uint8_t[]>(kMaxMessageSize + 1);
  {
    zx_iovec_t vec = {
        .buffer = large_buf.get(),
        .capacity = kMaxMessageSize + 1,
    };
    EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, &vec, 1), ZX_ERR_INVALID_ARGS);
  }

  // Exceeds buffer size.
  {
    zx_iovec_t vec = {
        .buffer = large_buf.get(),
        .capacity = zx_system_get_page_size() + 1,
    };
    EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, &vec, 1), ZX_ERR_NO_SPACE);
  }

  // Bad buffer.
  {
    zx_iovec_t vec = {
        .buffer = reinterpret_cast<void*>(1),
        .capacity = 5,
    };
    EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, &vec, 1), ZX_ERR_NOT_FOUND);
  }

  // Too many iovecs.
  {
    constexpr int kCount = 9;
    char buffer[] = "hello";
    zx_iovec_t vec[kCount];
    for (int i = 0; i < kCount; ++i) {
      vec[i] = {
          .buffer = buffer,
          .capacity = std::size(buffer),
      };
    }
    EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, vec, kCount), ZX_ERR_INVALID_ARGS);
  }

  // Make sure that we could actually write to ep0.
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));
}

TEST(Iob, IobWriteCorrupt) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  zx::result<MappingHelper> mapping =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size * 2);
  ASSERT_OK(mapping);

  uint64_t* phead = reinterpret_cast<uint64_t*>(mapping->addr());
  uint64_t* ptail = reinterpret_cast<uint64_t*>(mapping->addr() + 8);

  // Corrupted tail: tail exceeds head.
  *ptail = 8;

  char buffer[] = "12345678abc";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };

  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, &vec, 1), ZX_ERR_IO_DATA_INTEGRITY);

  // Misaligned head.
  *phead = 9;
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, &vec, 1), ZX_ERR_IO_DATA_INTEGRITY);

  // Overflow of head pointer. The message gets rounded up to nearest 8 bytes and there's a 16 byte
  // header.
  *phead = *ptail = 0xffff'ffff'ffff'ffff - 16 - 16 + 1;
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, &vec, 1), ZX_ERR_IO_DATA_INTEGRITY);

  // There should be room to write a smaller message.
  vec.capacity = 5;
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));
}

void IobWriteTest(bool use_two_iovecs) {
  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  *reinterpret_cast<zx_iob_discipline_mediated_write_ring_buffer_t*>(config.discipline.reserved) = {
      .tag = 0x0123456789abcdef,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  zx::result<MappingHelper> mapping =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size * 2);
  ASSERT_OK(mapping);

  uint64_t* phead = reinterpret_cast<uint64_t*>(mapping->addr());
  uint64_t* ptail = reinterpret_cast<uint64_t*>(mapping->addr() + 8);

  // Write some messages and check that we see the expected updates to the buffer.
  char buffer[] = "12345";
  zx_iovec_t vec[2];
  int iovec_count;

  if (use_two_iovecs) {
    constexpr int kOffset = 3;
    vec[0] = {
        .buffer = buffer,
        .capacity = kOffset,
    };
    vec[1] = {
        .buffer = buffer + kOffset,
        .capacity = std::size(buffer) - kOffset,
    };
    iovec_count = 2;
  } else {
    vec[0] = {
        .buffer = buffer,
        .capacity = std::size(buffer),
    };
    iovec_count = 1;
  }

  const uint8_t expected[] = {
      0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01,  // tag
      6,    0,    0,    0,    0,    0,    0,    0,     // length
      '1',  '2',  '3',  '4',  '5',  0,                 // message
  };
  constexpr size_t kRoundedMessageSize = 8 + 16;

  uint64_t last_head = 0;
  for (unsigned i = 0; i < page_size / kRoundedMessageSize; ++i) {
    EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, vec, iovec_count));

    EXPECT_EQ(*phead, last_head + 8 + 16);
    EXPECT_EQ(*ptail, 0);
    EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(mapping->addr() + page_size + last_head), expected,
                    std::size(expected));
    last_head = *phead;
  }

  // The next message we write should wrap the buffer, but there won't be any space.
  EXPECT_STATUS(zx_iob_writev(ep0.get(), 0, 0, vec, iovec_count), ZX_ERR_NO_SPACE);

  // Make some space, and then the next write should succeed.
  *ptail += kRoundedMessageSize;
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, vec, iovec_count));

  EXPECT_EQ(*phead, last_head + 8 + 16);
  EXPECT_EQ(*ptail, kRoundedMessageSize);

  size_t amount_before_wrapping = page_size - last_head;
  EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(mapping->addr() + page_size + last_head), expected,
                  amount_before_wrapping);
  EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(mapping->addr() + page_size),
                  expected + amount_before_wrapping, std::size(expected) - amount_before_wrapping);
}

TEST(Iob, IobWriteSuccess) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  IobWriteTest(false);
}

TEST(Iob, IobWriteWithMultipleVectors) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  IobWriteTest(true);
}

TEST(Iob, IobWriteWithFaultSucceeds) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // Create a VMO and map it. The first access to the mapping should trigger a page fault.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));

  zx_vaddr_t addr{0};
  ASSERT_OK(
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, page_size, &addr));

  auto clean_up = fit::defer([addr, page_size] { zx::vmar::root_self()->unmap(addr, page_size); });

  zx_iovec_t vec = {
      .buffer = reinterpret_cast<void*>(addr),
      .capacity = 10,
  };

  // This should trigger a page fault on the VMO we created.
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));
}

TEST(Iob, IobWriteUpdatedSignal) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(
      zx_object_wait_async(shared_region, port.get(), 0x5678, ZX_IOB_SHARED_REGION_UPDATED, 0));

  zx_port_packet_t packet;
  EXPECT_STATUS(port.wait(zx::time::infinite_past(), &packet), ZX_ERR_TIMED_OUT);

  char buffer[] = "12345";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = std::size(buffer),
  };
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));

  EXPECT_OK(port.wait(zx::time::infinite_past(), &packet));

  EXPECT_EQ(packet.key, 0x5678);
  EXPECT_EQ(packet.type, ZX_PKT_TYPE_SIGNAL_ONE);
  EXPECT_EQ(packet.signal.trigger, ZX_IOB_SHARED_REGION_UPDATED);
  EXPECT_EQ(packet.signal.observed, ZX_IOB_SHARED_REGION_UPDATED);
}

TEST(Iob, IobWriteMaxMessageSize) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  constexpr size_t kMaxMessageSize = 65535 - 16;
  // Chosen so the buffer can fit two maximum sized messages.
  const uint64_t page_size = zx_system_get_page_size();
  const size_t region_size = 2 * (kMaxMessageSize / page_size + 2) * page_size;

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, region_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  *reinterpret_cast<zx_iob_discipline_mediated_write_ring_buffer_t*>(config.discipline.reserved) = {
      .tag = 0x0123456789abcdef,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  zx::result<MappingHelper> mapping =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, region_size);
  ASSERT_OK(mapping);

  uint64_t* phead = reinterpret_cast<uint64_t*>(mapping->addr());

  auto buffer = std::make_unique<uint8_t[]>(kMaxMessageSize);

  for (size_t i = 0; i < kMaxMessageSize; ++i) {
    buffer.get()[i] = static_cast<uint8_t>(i);
  }

  zx_iovec_t vec = {
      .buffer = buffer.get(),
      .capacity = kMaxMessageSize,
  };

  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));

  const uint8_t expected[] = {
      // tag
      0xef,
      0xcd,
      0xab,
      0x89,
      0x67,
      0x45,
      0x23,
      0x01,
      // length
      static_cast<uint8_t>(kMaxMessageSize),
      static_cast<uint8_t>(kMaxMessageSize >> 8),
      0,
      0,
      0,
      0,
      0,
      0,
  };

  // Check the expected data.
  EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(mapping->addr() + page_size), expected,
                  std::size(expected));
  EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(mapping->addr() + page_size + std::size(expected)),
                  buffer.get(), kMaxMessageSize);

  const uint64_t head = *phead;
  EXPECT_EQ(head, (kMaxMessageSize + 16 + 7) & ~6);

  // Do the same again, but this time use the maximum number of iovecs.
  constexpr size_t kMaxIovecs = 8;
  zx_iovec_t vecs[kMaxIovecs];
  uint8_t* buf = buffer.get();
  size_t remaining = kMaxMessageSize;
  size_t gap = kMaxMessageSize / kMaxIovecs;
  for (size_t i = 0; i < kMaxIovecs - 1; ++i) {
    vecs[i] = {
        .buffer = buf,
        .capacity = gap,
    };
    buf += gap;
    remaining -= gap;
  }
  vecs[kMaxIovecs - 1] = {
      .buffer = buf,
      .capacity = remaining,
  };

  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, vecs, kMaxIovecs));

  EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(mapping->addr() + page_size + head), expected,
                  std::size(expected));
  EXPECT_BYTES_EQ(
      reinterpret_cast<uint8_t*>(mapping->addr() + page_size + head + std::size(expected)),
      buffer.get(), kMaxMessageSize);
}

TEST(Iob, IobWriteMultiThreaded) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };
  *reinterpret_cast<zx_iob_discipline_mediated_write_ring_buffer_t*>(config.discipline.reserved) = {
      .tag = 0x0123456789abcdef,
  };
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  constexpr int kIterations = 1000;
  auto writer_thread = [&](zx_iovec_t& vec) {
    for (int i = 0; i < kIterations; ++i) {
      for (;;) {
        zx_status_t status = zx_iob_writev(ep0.get(), 0, 0, &vec, 1);
        if (status == ZX_OK)
          break;
        EXPECT_EQ(status, ZX_ERR_NO_SPACE);
        usleep(1000);
      }
    }
  };

  // Start two writer threads.
  std::thread thread1([&] {
    char buffer[] = "12345";
    zx_iovec_t vec = {
        .buffer = buffer,
        .capacity = std::size(buffer),
    };
    writer_thread(vec);
  });

  std::thread thread2([&] {
    char buffer[] = "67890abcde";
    zx_iovec_t vec = {
        .buffer = buffer,
        .capacity = std::size(buffer),
    };
    writer_thread(vec);
  });

  zx::result<MappingHelper> mapping =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size * 2);
  ASSERT_OK(mapping);

  std::atomic_ref<uint64_t> phead(*reinterpret_cast<uint64_t*>(mapping->addr()));
  std::atomic_ref<uint64_t> ptail(*reinterpret_cast<uint64_t*>(mapping->addr() + 8));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  // Read and verify the messages.
  int thread1_message_count = 0, thread2_message_count = 0;
  uint64_t tail = 0;
  while (thread1_message_count < kIterations || thread2_message_count < kIterations) {
    uint64_t head;

    for (;;) {
      head = phead.load(std::memory_order_acquire);
      if (head > tail)
        break;
      ASSERT_OK(
          zx_object_wait_async(shared_region, port.get(), 0x5678, ZX_IOB_SHARED_REGION_UPDATED, 0));
      head = phead.load(std::memory_order_acquire);
      if (head > tail)
        break;
      zx_port_packet_t packet;
      EXPECT_OK(port.wait(zx::time::infinite(), &packet));
    }

    while (tail < head) {
      // Tag
      const uint8_t expected_tag[] = {0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01};
      EXPECT_BYTES_EQ(reinterpret_cast<uint64_t*>(mapping->addr() + page_size + tail % page_size),
                      expected_tag, 8);

      // Read the message length to determine which of the two messages we expect.
      uint64_t message_len =
          *reinterpret_cast<uint64_t*>(mapping->addr() + page_size + (tail + 8) % page_size);
      if (message_len == 6) {
        EXPECT_BYTES_EQ(
            reinterpret_cast<uint8_t*>(mapping->addr() + page_size + (tail + 16) % page_size),
            "12345", 6);
        ++thread1_message_count;
        tail += 16 + 8;
      } else {
        EXPECT_EQ(message_len, 11);
        EXPECT_BYTES_EQ(
            reinterpret_cast<uint8_t*>(mapping->addr() + page_size + (tail + 16) % page_size),
            "67890abc", 8);
        EXPECT_BYTES_EQ(
            reinterpret_cast<uint8_t*>(mapping->addr() + page_size + (tail + 24) % page_size), "de",
            3);
        ++thread2_message_count;
        tail += 16 + 16;
      }
    }
    EXPECT_EQ(head, tail);

    ptail.store(tail, std::memory_order_release);
  }

  thread1.join();
  thread2.join();

  EXPECT_EQ(phead, ptail);
}

TEST(Iob, VmoKoid) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();

  zx::iob ep0, ep1;

  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  zx_iob_region_t config = {
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region,
  };

  // Capture the process VMOs before creating the IOBuffer.
  std::vector<zx_info_vmo> vmos(16);

  auto read_process_vmos = [&] {
    for (;;) {
      size_t actual, avail;
      ASSERT_OK(zx::process::self()->get_info(ZX_INFO_PROCESS_VMOS, vmos.data(),
                                              vmos.size() * sizeof(zx_info_vmo), &actual, &avail));
      if (actual == avail) {
        break;
      }
      vmos.resize(avail + 16);
    }
  };

  read_process_vmos();

  // Record all the unique koids we find.
  std::unordered_set<zx_koid_t> koids;
  for (const auto& vmo : vmos) {
    koids.insert(vmo.koid);
  }

  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  zx_iob_region_info_t region_info;
  size_t actual, avail;
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, &region_info, sizeof(region_info), &actual, &avail));
  ASSERT_EQ(actual, 1);

  // It should be a new koid.
  EXPECT_FALSE(koids.contains(region_info.koid));
  koids.insert(region_info.koid);

  const zx_koid_t shared_region_koid = region_info.koid;

  // Makes sure it's reported in the process list.
  read_process_vmos();
  bool found = false;
  for (const auto& vmo : vmos) {
    if (vmo.koid == region_info.koid) {
      found = true;
      break;
    }
  }
  EXPECT_TRUE(found);

  // Create many more IOBuffers, and check that they all use the same koid.
  std::vector<std::pair<zx::iob, zx::iob>> iobs;
  for (int i = 0; i < 500; ++i) {
    zx::iob ep0, ep1;
    ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

    zx_iob_region_info_t region_info;
    size_t actual, avail;
    ASSERT_OK(
        ep0.get_info(ZX_INFO_IOB_REGIONS, &region_info, sizeof(region_info), &actual, &avail));
    ASSERT_EQ(actual, 1);
    EXPECT_EQ(region_info.koid, shared_region_koid);

    ASSERT_OK(
        ep1.get_info(ZX_INFO_IOB_REGIONS, &region_info, sizeof(region_info), &actual, &avail));
    ASSERT_EQ(actual, 1);
    EXPECT_EQ(region_info.koid, shared_region_koid);

    iobs.push_back(std::make_pair(std::move(ep0), std::move(ep1)));
  }

  // Check that the number of new process VMOs has not grown unexpectedly.
  read_process_vmos();
  int new_vmos = 0;
  for (const auto& vmo : vmos) {
    if (!koids.contains(vmo.koid)) {
      ++new_vmos;
      koids.insert(vmo.koid);
    }
  }

  // Allow for some extra VMOs due to allocations.
  EXPECT_LT(new_vmos, 20);
}

// Regression test for b/504721827.
TEST(Iob, VmarMapIobWithFaultBeyondStreamSizeReturnsError) {
  zx::iob ep0, ep1;
  zx_iob_region_t region = {};
  region.type = ZX_IOB_REGION_TYPE_PRIVATE;
  region.access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE;
  region.size = 0x1000;
  region.discipline.type = ZX_IOB_DISCIPLINE_TYPE_NONE;
  region.private_region.options = 0;

  ASSERT_OK(zx::iob::create(0, &region, 1, &ep0, &ep1));

  zx_vaddr_t addr = 0;
  // This used to crash the kernel due to a missing stream size manager.
  // With the feature disabled, it should return ZX_ERR_INVALID_ARGS.
  zx_status_t status = zx::vmar::root_self()->map_iob(
      ZX_VM_PERM_READ | ZX_VM_MAP_RANGE | ZX_VM_FAULT_BEYOND_STREAM_SIZE | ZX_VM_ALLOW_FAULTS, 0,
      ep0, 0, 0, 0x1000, &addr);

  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
}

TEST(Iob, CreateMaxRegions) {
  const uint64_t page_size = zx_system_get_page_size();
  std::array<zx_iob_region_t, ZX_IOB_MAX_REGIONS> configs{};

  for (size_t i = 0; i < ZX_IOB_MAX_REGIONS; ++i) {
    if (i % 2 == 0) {
      configs[i] = zx_iob_region_t{
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size * ((i % 4) + 1),
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      };
    } else {
      configs[i] = zx_iob_region_t{
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEp0OnlyRwMap | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
          .private_region = {.options = 0},
      };
    }
  }

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, configs.data(), configs.size(), &ep0, &ep1));

  zx_info_iob_t info;
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.options, 0);
  EXPECT_EQ(info.region_count, ZX_IOB_MAX_REGIONS);

  std::array<zx_iob_region_info_t, ZX_IOB_MAX_REGIONS> region_infos;
  size_t actual = 0, available = 0;
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, region_infos.data(), sizeof(region_infos), &actual,
                         &available));
  EXPECT_EQ(actual, ZX_IOB_MAX_REGIONS);
  EXPECT_EQ(available, ZX_IOB_MAX_REGIONS);

  // Map first region (index 0) from both endpoints and verify communication.
  zx::result<MappingHelper> map_first =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(map_first.status_value());
  zx::result<MappingHelper> map_first_peer =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep1, 0, 0, page_size);
  ASSERT_OK(map_first_peer.status_value());

  const char* msg = "Region0MaxTest";
  memcpy(reinterpret_cast<void*>(map_first->addr()), msg, strlen(msg) + 1);
  EXPECT_STREQ(reinterpret_cast<const char*>(map_first_peer->addr()), msg);

  // Map last region (index 63 is ID_ALLOCATOR, ep0 can map rw).
  zx::result<MappingHelper> map_last =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 63, 0, page_size);
  ASSERT_OK(map_last.status_value());
}

TEST(Iob, CreateOutOfRangeAndInvalidArgs) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();
  zx_handle_t ep0 = ZX_HANDLE_INVALID, ep1 = ZX_HANDLE_INVALID;

  // Too many regions: ZX_IOB_MAX_REGIONS + 1
  std::array<zx_iob_region_t, ZX_IOB_MAX_REGIONS + 1> too_many_configs{};
  for (size_t i = 0; i < too_many_configs.size(); ++i) {
    too_many_configs[i] = zx_iob_region_t{
        .type = ZX_IOB_REGION_TYPE_PRIVATE,
        .access = kIoBufferEpRwMap,
        .size = page_size,
        .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
        .private_region = {.options = 0},
    };
  }
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx_iob_create(0, too_many_configs.data(), too_many_configs.size(), &ep0, &ep1));

  // Non-zero options
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(1, too_many_configs.data(), 1, &ep0, &ep1));

  // Zero size for private region creates a 0-byte region VMO successfully
  zx_iob_region_t zero_size_config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = 0,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };
  EXPECT_OK(zx_iob_create(0, &zero_size_config, 1, &ep0, &ep1));
  EXPECT_OK(zx_handle_close(ep0));
  EXPECT_OK(zx_handle_close(ep1));

  // Unknown region type
  zx_iob_region_t bad_type_config{
      .type = 42,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &bad_type_config, 1, &ep0, &ep1));

  // Unknown discipline type
  zx_iob_region_t bad_disc_config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = 42},
      .private_region = {.options = 0},
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &bad_disc_config, 1, &ep0, &ep1));

  // Read-only access (no write mapping, no mediated access)
  zx_iob_region_t ro_config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP1_CAN_MAP_READ,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &ro_config, 1, &ep0, &ep1));

  // DISCIPLINE_TYPE_NONE with mediated access
  zx_iob_region_t none_with_med_config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap | ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &none_with_med_config, 1, &ep0, &ep1));

  // DISCIPLINE_TYPE_ID_ALLOCATOR with read-only mediated access (no mediated write)
  zx_iob_region_t id_med_ro_config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEp0OnlyRwMap | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
      .private_region = {.options = 0},
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &id_med_ro_config, 1, &ep0, &ep1));

  // Shared region with invalid handle (ZX_HANDLE_INVALID)
  zx_iob_region_t shared_bad_handle_config{
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(shared_bad_handle_config.max_extension) = {
      .shared_region = ZX_HANDLE_INVALID,
  };
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_iob_create(0, &shared_bad_handle_config, 1, &ep0, &ep1));

  // Shared region with non-zero options
  zx_iob_region_t shared_bad_options_config{
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(shared_bad_options_config.max_extension) = {
      .options = 1,
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_create(0, &shared_bad_options_config, 1, &ep0, &ep1));

  // Shared region with wrong handle type (e.g. channel)
  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));
  reinterpret_cast<zx_iob_region_shared_t&>(shared_bad_handle_config.max_extension) = {
      .shared_region = ch0.get(),
  };
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_iob_create(0, &shared_bad_handle_config, 1, &ep0, &ep1));
}

TEST(Iob, HandleBasicInfoAndRights) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  zx_info_handle_basic_t ep0_basic{}, ep1_basic{};
  ASSERT_OK(ep0.get_info(ZX_INFO_HANDLE_BASIC, &ep0_basic, sizeof(ep0_basic), nullptr, nullptr));
  ASSERT_OK(ep1.get_info(ZX_INFO_HANDLE_BASIC, &ep1_basic, sizeof(ep1_basic), nullptr, nullptr));

  EXPECT_EQ(ep0_basic.type, ZX_OBJ_TYPE_IOB);
  EXPECT_EQ(ep1_basic.type, ZX_OBJ_TYPE_IOB);
  EXPECT_EQ(ep0_basic.rights, ZX_DEFAULT_IOB_RIGHTS);
  EXPECT_EQ(ep1_basic.rights, ZX_DEFAULT_IOB_RIGHTS);
  EXPECT_NE(ep0_basic.koid, ZX_KOID_INVALID);
  EXPECT_NE(ep1_basic.koid, ZX_KOID_INVALID);
  EXPECT_NE(ep0_basic.koid, ep1_basic.koid);
  EXPECT_EQ(ep0_basic.related_koid, ep1_basic.koid);
  EXPECT_EQ(ep1_basic.related_koid, ep0_basic.koid);

  // Test replacing handle with reduced rights
  zx::iob reduced_ep0;
  ASSERT_OK(ep0.replace(ZX_RIGHT_READ | ZX_RIGHT_MAP, &reduced_ep0));
  zx_info_handle_basic_t reduced_basic{};
  ASSERT_OK(reduced_ep0.get_info(ZX_INFO_HANDLE_BASIC, &reduced_basic, sizeof(reduced_basic),
                                 nullptr, nullptr));
  EXPECT_EQ(reduced_basic.rights, ZX_RIGHT_READ | ZX_RIGHT_MAP);
  EXPECT_EQ(reduced_basic.koid, ep0_basic.koid);
}

TEST(Iob, SignalsAndSignalPeer) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // Signal self
  zx_signals_t observed = 0;
  ASSERT_OK(ep0.signal(0, ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1));
  ASSERT_OK(ep0.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &observed));
  EXPECT_EQ(observed & (ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1), ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1);

  // ep1 should not observe ep0's self signal
  EXPECT_EQ(ZX_ERR_TIMED_OUT, ep1.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &observed));

  // Clear signal on ep0
  ASSERT_OK(ep0.signal(ZX_USER_SIGNAL_0, 0));
  ASSERT_OK(ep0.wait_one(ZX_USER_SIGNAL_1, zx::time::infinite_past(), &observed));
  EXPECT_EQ(observed & (ZX_USER_SIGNAL_0 | ZX_USER_SIGNAL_1), ZX_USER_SIGNAL_1);

  // Signal peer
  ASSERT_OK(zx_object_signal_peer(ep0.get(), 0, ZX_USER_SIGNAL_2));
  ASSERT_OK(ep1.wait_one(ZX_USER_SIGNAL_2, zx::time::infinite_past(), &observed));
  EXPECT_TRUE(observed & ZX_USER_SIGNAL_2);
  EXPECT_EQ(ZX_ERR_TIMED_OUT, ep0.wait_one(ZX_USER_SIGNAL_2, zx::time::infinite_past(), &observed));

  // Clear signal on peer
  ASSERT_OK(zx_object_signal_peer(ep0.get(), ZX_USER_SIGNAL_2, 0));
  EXPECT_EQ(ZX_ERR_TIMED_OUT, ep1.wait_one(ZX_USER_SIGNAL_2, zx::time::infinite_past(), &observed));

  // Rights enforcement on signal operations
  zx::iob no_signal_ep0, no_signal_peer_ep0;
  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_SIGNAL, &no_signal_ep0));
  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_SIGNAL_PEER, &no_signal_peer_ep0));

  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, no_signal_ep0.signal(0, ZX_USER_SIGNAL_3));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            zx_object_signal_peer(no_signal_peer_ep0.get(), 0, ZX_USER_SIGNAL_3));

  // Close peer and test signal_peer error
  ep1.reset();
  EXPECT_EQ(ZX_ERR_PEER_CLOSED, zx_object_signal_peer(ep0.get(), 0, ZX_USER_SIGNAL_3));

  // Reserved kernel signals cannot be set/cleared by user
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, ep0.signal(0, ZX_IOB_PEER_CLOSED));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, ep0.signal(ZX_IOB_PEER_CLOSED, 0));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_object_signal_peer(ep0.get(), 0, ZX_IOB_PEER_CLOSED));
}

TEST(Iob, SymmetricPeerClosedWithMappings) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // Create mapping from ep1
  zx::result<MappingHelper> region =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep1, 0, 0, page_size);
  ASSERT_OK(region.status_value());

  // Close ep1 handle
  ep1.reset();

  // ep0 should not see PEER_CLOSED yet
  zx_signals_t observed = 0;
  EXPECT_EQ(ZX_ERR_TIMED_OUT, ep0.wait_one(ZX_IOB_PEER_CLOSED, zx::time{0}, &observed));
  EXPECT_EQ(0, observed);

  // Unmap ep1's region
  ASSERT_OK(region->Unmap());

  // Wait for ep0 to observe PEER_CLOSED
  while (true) {
    zx::time deadline = zx::time(zx_deadline_after(ZX_SEC(5)));
    zx_status_t status = ep0.wait_one(ZX_IOB_PEER_CLOSED, deadline, &observed);
    if (status != ZX_ERR_TIMED_OUT) {
      ASSERT_OK(status);
      break;
    }
  }
  EXPECT_EQ(ZX_IOB_PEER_CLOSED, observed);
}

TEST(Iob, PartialAndMultipleMappings) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = kIoBufferEpRwMap,
      .size = 4 * page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
      .private_region = {.options = 0},
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // Map page 0 from ep0
  zx::result<MappingHelper> ep0_page0 =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, page_size);
  ASSERT_OK(ep0_page0.status_value());

  // Map page 2 from ep0
  zx::result<MappingHelper> ep0_page2 = MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                                                              ep0, 0, 2 * page_size, page_size);
  ASSERT_OK(ep0_page2.status_value());

  // Map page 2 from ep1
  zx::result<MappingHelper> ep1_page2 = MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                                                              ep1, 0, 2 * page_size, page_size);
  ASSERT_OK(ep1_page2.status_value());

  // Write to page 2 via ep0 mapping
  const char* str = "PartialMappingData";
  memcpy(reinterpret_cast<void*>(ep0_page2->addr()), str, strlen(str) + 1);
  EXPECT_STREQ(reinterpret_cast<const char*>(ep1_page2->addr()), str);

  // Map the full 4 pages from ep0 simultaneously
  zx::result<MappingHelper> ep0_full =
      MappingHelper::Create(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, 4 * page_size);
  ASSERT_OK(ep0_full.status_value());

  // Verify full map sees the write at page 2 offset
  EXPECT_STREQ(reinterpret_cast<const char*>(ep0_full->addr() + 2 * page_size), str);

  // Write to full map at page 0 offset, verify ep0_page0 sees it
  const char* str0 = "PageZeroData";
  memcpy(reinterpret_cast<void*>(ep0_full->addr()), str0, strlen(str0) + 1);
  EXPECT_STREQ(reinterpret_cast<const char*>(ep0_page0->addr()), str0);

  // Unmap partial mappings, verify full map still accessible
  ASSERT_OK(ep0_page0->Unmap());
  ASSERT_OK(ep0_page2->Unmap());
  EXPECT_STREQ(reinterpret_cast<const char*>(ep0_full->addr() + 2 * page_size), str);
}

TEST(Iob, VmarMapIobErrors) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config[2]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 2 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
          .private_region = {.options = 0},
      },
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, config, 2, &ep0, &ep1));

  zx_vaddr_t addr = 0;
  zx::unowned_vmar vmar = zx::vmar::root_self();

  // Invalid VMAR handle
  EXPECT_EQ(ZX_ERR_BAD_HANDLE,
            zx_vmar_map_iob(ZX_HANDLE_INVALID, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0.get(), 0,
                            0, page_size, &addr));

  // Wrong VMAR handle type (passing IOB handle as VMAR)
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_vmar_map_iob(ep0.get(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                                               ep0.get(), 0, 0, page_size, &addr));

  // Invalid IOB handle
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_vmar_map_iob(vmar->get(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                                               ZX_HANDLE_INVALID, 0, 0, page_size, &addr));

  // Wrong IOB handle type (passing VMAR handle as IOB)
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_vmar_map_iob(vmar->get(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                                               vmar->get(), 0, 0, page_size, &addr));

  // Out of range region index
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 2, 0, page_size, &addr));

  // Unaligned region_offset
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 1, page_size, &addr));

  // Unaligned vmar_offset with ZX_VM_SPECIFIC
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC,
                                               1, ep0, 0, 0, page_size, &addr));

  // Zero region_length
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0, 0, 0, &addr));

  // Region offset + length overflow
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 0,
                          std::numeric_limits<size_t>::max() - page_size + 1, page_size, &addr));

  // Mapping region 1 which has no map access
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, ep0, 1, 0, page_size, &addr));

  // Handle without ZX_RIGHT_READ requesting read
  zx::iob no_read_ep0;
  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_READ, &no_read_ep0));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            vmar->map_iob(ZX_VM_PERM_READ, 0, no_read_ep0, 0, 0, page_size, &addr));

  // Handle without ZX_RIGHT_WRITE requesting write
  zx::iob no_write_ep0;
  ASSERT_OK(ep0.duplicate(ZX_DEFAULT_IOB_RIGHTS & ~ZX_RIGHT_WRITE, &no_write_ep0));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, no_write_ep0,
                                                0, 0, page_size, &addr));

  // Requesting execute permissions (not supported for IOBs)
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            vmar->map_iob(ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, 0, ep0, 0, 0, page_size, &addr));
}

TEST(Iob, IdAllocatorMultipleEndpointsAndZeroBlob) {
  NEEDS_NEXT_SKIP(zx_iob_allocate_id);

  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_PRIVATE,
      .access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE |
                ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE,
      .size = page_size,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR},
      .private_region = {.options = 0},
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // Allocate ID with 0-byte blob from ep0
  uint32_t id0 = 999;
  EXPECT_OK(zx_iob_allocate_id(ep0.get(), 0, 0, nullptr, 0, &id0));
  EXPECT_EQ(id0, 0u);

  // Allocate ID with 10-byte blob from ep1
  uint8_t blob1[10] = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10};
  uint32_t id1 = 999;
  EXPECT_OK(zx_iob_allocate_id(ep1.get(), 0, 0, blob1, sizeof(blob1), &id1));
  EXPECT_EQ(id1, 1u);

  // Allocate ID with 0-byte blob from ep1
  uint32_t id2 = 999;
  EXPECT_OK(zx_iob_allocate_id(ep1.get(), 0, 0, nullptr, 0, &id2));
  EXPECT_EQ(id2, 2u);

  // Allocate ID with 20-byte blob from ep0
  uint8_t blob3[20] = {};
  uint32_t id3 = 999;
  EXPECT_OK(zx_iob_allocate_id(ep0.get(), 0, 0, blob3, sizeof(blob3), &id3));
  EXPECT_EQ(id3, 3u);

  // Invalid handle error
  EXPECT_EQ(ZX_ERR_BAD_HANDLE,
            zx_iob_allocate_id(ZX_HANDLE_INVALID, 0, 0, blob1, sizeof(blob1), &id0));

  // Wrong handle type (e.g. channel)
  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_iob_allocate_id(ch0.get(), 0, 0, blob1, sizeof(blob1), &id0));

  // Null id pointer
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            zx_iob_allocate_id(ep0.get(), 0, 0, blob1, sizeof(blob1), nullptr));

  // Invalid blob pointer when blob_size > 0
  EXPECT_EQ(
      ZX_ERR_INVALID_ARGS,
      zx_iob_allocate_id(ep0.get(), 0, 0, reinterpret_cast<const void*>(1), sizeof(blob1), &id0));
}

TEST(Iob, WritevZeroLengthAndMultipleIobs) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();
  zx_handle_t shared_region;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &shared_region));
  [[maybe_unused]] auto clean_up_shared_region = [=] { zx_handle_close(shared_region); };

  // Create IOB pair A with tag A
  constexpr uint64_t kTagA = 0xAAAA'AAAA'AAAA'AAAA;
  zx_iob_region_t config_a{
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config_a.max_extension) = {
      .shared_region = shared_region,
  };
  *reinterpret_cast<zx_iob_discipline_mediated_write_ring_buffer_t*>(
      config_a.discipline.reserved) = {
      .tag = kTagA,
  };
  zx::iob ep0_a, ep1_a;
  ASSERT_OK(zx::iob::create(0, &config_a, 1, &ep0_a, &ep1_a));

  // Create IOB pair B with tag B
  constexpr uint64_t kTagB = 0xBBBB'BBBB'BBBB'BBBB;
  zx_iob_region_t config_b{
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config_b.max_extension) = {
      .shared_region = shared_region,
  };
  *reinterpret_cast<zx_iob_discipline_mediated_write_ring_buffer_t*>(
      config_b.discipline.reserved) = {
      .tag = kTagB,
  };
  zx::iob ep0_b, ep1_b;
  ASSERT_OK(zx::iob::create(0, &config_b, 1, &ep0_b, &ep1_b));

  // Write 0-byte message with vector_count = 0
  EXPECT_OK(zx_iob_writev(ep0_a.get(), 0, 0, nullptr, 0));

  // Write 0-byte message with vector_count = 1, capacity = 0
  zx_iovec_t empty_vec{
      .buffer = nullptr,
      .capacity = 0,
  };
  EXPECT_OK(zx_iob_writev(ep0_b.get(), 0, 0, &empty_vec, 1));

  // Write non-empty message from A
  char msg_a[] = "MsgFromA";
  zx_iovec_t vec_a{
      .buffer = msg_a,
      .capacity = sizeof(msg_a),
  };
  EXPECT_OK(zx_iob_writev(ep0_a.get(), 0, 0, &vec_a, 1));

  // Write non-empty message from B
  char msg_b[] = "MsgFromB!";
  zx_iovec_t vec_b{
      .buffer = msg_b,
      .capacity = sizeof(msg_b),
  };
  EXPECT_OK(zx_iob_writev(ep0_b.get(), 0, 0, &vec_b, 1));

  // Map and verify ring buffer contents
  zx::result<MappingHelper> mapping =
      MappingHelper::Create(ZX_VM_PERM_READ, 0, ep0_a, 0, 0, 2 * page_size);
  ASSERT_OK(mapping);

  uint64_t* phead = reinterpret_cast<uint64_t*>(mapping->addr());
  uint64_t* ptail = reinterpret_cast<uint64_t*>(mapping->addr() + 8);
  EXPECT_EQ(*ptail, 0u);

  // Message 0: 0 bytes from A. Size = 16 (tag + len=0).
  // Message 1: 0 bytes from B. Size = 16 (tag + len=0).
  // Message 2: sizeof(msg_a)=9 rounded to 16 + 16 header = 32.
  // Message 3: sizeof(msg_b)=10 rounded to 16 + 16 header = 32.
  // Total head = 16 + 16 + 32 + 32 = 96.
  EXPECT_EQ(*phead, 96u);

  // Verify Message 0
  uint64_t* m0_tag = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 0);
  uint64_t* m0_len = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 8);
  EXPECT_EQ(*m0_tag, kTagA);
  EXPECT_EQ(*m0_len, 0u);

  // Verify Message 1
  uint64_t* m1_tag = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 16);
  uint64_t* m1_len = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 24);
  EXPECT_EQ(*m1_tag, kTagB);
  EXPECT_EQ(*m1_len, 0u);

  // Verify Message 2
  uint64_t* m2_tag = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 32);
  uint64_t* m2_len = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 40);
  EXPECT_EQ(*m2_tag, kTagA);
  EXPECT_EQ(*m2_len, sizeof(msg_a));
  EXPECT_BYTES_EQ(reinterpret_cast<const void*>(mapping->addr() + page_size + 48), msg_a,
                  sizeof(msg_a));

  // Verify Message 3
  uint64_t* m3_tag = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 64);
  uint64_t* m3_len = reinterpret_cast<uint64_t*>(mapping->addr() + page_size + 72);
  EXPECT_EQ(*m3_tag, kTagB);
  EXPECT_EQ(*m3_len, sizeof(msg_b));
  EXPECT_BYTES_EQ(reinterpret_cast<const void*>(mapping->addr() + page_size + 80), msg_b,
                  sizeof(msg_b));

  // Error cases
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_iob_writev(ZX_HANDLE_INVALID, 0, 0, &vec_a, 1));
  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_iob_writev(ch0.get(), 0, 0, &vec_a, 1));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_iob_writev(ep0_a.get(), 0, 0, nullptr, 1));
}

TEST(IobSharedRegion, HandleInfoAndLifetime) {
  NEEDS_NEXT_SKIP(zx_iob_create_shared_region);

  const uint64_t page_size = zx_system_get_page_size();
  zx_handle_t raw_handle;
  ASSERT_OK(zx_iob_create_shared_region(0, 2 * page_size, &raw_handle));
  zx::handle shared_region(raw_handle);

  // Check handle info
  zx_info_handle_basic_t basic{};
  ASSERT_OK(shared_region.get_info(ZX_INFO_HANDLE_BASIC, &basic, sizeof(basic), nullptr, nullptr));
  EXPECT_EQ(basic.type, ZX_OBJ_TYPE_IOB_SHARED_REGION);
  EXPECT_EQ(basic.rights, ZX_DEFAULT_IOB_SHARED_REGION_RIGHTS);
  EXPECT_NE(basic.koid, ZX_KOID_INVALID);
  EXPECT_EQ(basic.related_koid, ZX_KOID_INVALID);

  // User signals on shared region
  zx_signals_t observed = 0;
  ASSERT_OK(shared_region.signal(0, ZX_USER_SIGNAL_0));
  ASSERT_OK(shared_region.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &observed));
  EXPECT_TRUE(observed & ZX_USER_SIGNAL_0);
  ASSERT_OK(shared_region.signal(ZX_USER_SIGNAL_0, 0));
  EXPECT_EQ(ZX_ERR_TIMED_OUT,
            shared_region.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &observed));

  // Create IOB and test that closing shared_region handle keeps IOB functional
  zx_iob_region_t config{
      .type = ZX_IOB_REGION_TYPE_SHARED,
      .access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | kIoBufferEpRwMap,
      .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER},
  };
  reinterpret_cast<zx_iob_region_shared_t&>(config.max_extension) = {
      .shared_region = shared_region.get(),
  };
  *reinterpret_cast<zx_iob_discipline_mediated_write_ring_buffer_t*>(config.discipline.reserved) = {
      .tag = 0x1234,
  };
  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, &config, 1, &ep0, &ep1));

  // Close shared_region handle
  shared_region.reset();

  // Writing to IOB still works
  char msg[] = "AliveAfterClose";
  zx_iovec_t vec{
      .buffer = msg,
      .capacity = sizeof(msg),
  };
  EXPECT_OK(zx_iob_writev(ep0.get(), 0, 0, &vec, 1));

  // Mapping still works
  zx::result<MappingHelper> mapping =
      MappingHelper::Create(ZX_VM_PERM_READ, 0, ep0, 0, 0, 2 * page_size);
  ASSERT_OK(mapping);
  uint64_t* phead = reinterpret_cast<uint64_t*>(mapping->addr());
  EXPECT_GT(*phead, 0u);
}

TEST(Iob, GetSetNamesEdgeCases) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config[2]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 2 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, config, 2, &ep0, &ep1));

  // Set empty name
  EXPECT_OK(ep0.set_property(ZX_PROP_NAME, "", 0));
  char name_buf[ZX_MAX_NAME_LEN] = {};
  EXPECT_OK(ep0.get_property(ZX_PROP_NAME, name_buf, sizeof(name_buf)));
  EXPECT_STREQ(name_buf, "");
  EXPECT_OK(ep1.get_property(ZX_PROP_NAME, name_buf, sizeof(name_buf)));
  EXPECT_STREQ(name_buf, "");

  // Set max length name
  char max_name[ZX_MAX_NAME_LEN];
  memset(max_name, 'A', ZX_MAX_NAME_LEN - 1);
  max_name[ZX_MAX_NAME_LEN - 1] = '\0';
  EXPECT_OK(ep1.set_property(ZX_PROP_NAME, max_name, strlen(max_name)));
  EXPECT_OK(ep0.get_property(ZX_PROP_NAME, name_buf, sizeof(name_buf)));
  EXPECT_STREQ(name_buf, max_name);

  // Check process VMOs to ensure both private regions have the name set
  size_t vmo_count = 0;
  ASSERT_OK(
      zx_object_get_info(zx_process_self(), ZX_INFO_PROCESS_VMOS, nullptr, 0, nullptr, &vmo_count));
  auto vmo_infos = std::make_unique<zx_info_vmo_t[]>(vmo_count);
  ASSERT_OK(zx_object_get_info(zx_process_self(), ZX_INFO_PROCESS_VMOS, vmo_infos.get(),
                               sizeof(zx_info_vmo_t) * vmo_count, nullptr, nullptr));

  size_t matching_vmos = 0;
  for (size_t i = 0; i < vmo_count; ++i) {
    if (strcmp(vmo_infos[i].name, max_name) == 0 &&
        (vmo_infos[i].flags & ZX_INFO_VMO_VIA_IOB_HANDLE)) {
      matching_vmos++;
    }
  }
  EXPECT_GE(matching_vmos, 2u);
}

TEST(Iob, GetInfoIobRegionsEdgeCases) {
  const uint64_t page_size = zx_system_get_page_size();
  zx_iob_region_t config[3]{
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 2 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
      {
          .type = ZX_IOB_REGION_TYPE_PRIVATE,
          .access = kIoBufferEpRwMap,
          .size = 3 * page_size,
          .discipline = zx_iob_discipline_t{.type = ZX_IOB_DISCIPLINE_TYPE_NONE},
          .private_region = {.options = 0},
      },
  };

  zx::iob ep0, ep1;
  ASSERT_OK(zx::iob::create(0, config, 3, &ep0, &ep1));

  // Query with nullptr buffer
  size_t actual = 999, avail = 999;
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, nullptr, 0, &actual, &avail));
  EXPECT_EQ(actual, 0u);
  EXPECT_EQ(avail, 3u);

  // Query with buffer for 1 entry
  zx_iob_region_info_t info[3]{};
  ASSERT_OK(
      ep0.get_info(ZX_INFO_IOB_REGIONS, info, sizeof(zx_iob_region_info_t) * 1, &actual, &avail));
  EXPECT_EQ(actual, 1u);
  EXPECT_EQ(avail, 3u);
  EXPECT_EQ(info[0].region.size, page_size);

  // Query with buffer for 2 entries
  ASSERT_OK(
      ep0.get_info(ZX_INFO_IOB_REGIONS, info, sizeof(zx_iob_region_info_t) * 2, &actual, &avail));
  EXPECT_EQ(actual, 2u);
  EXPECT_EQ(avail, 3u);
  EXPECT_EQ(info[0].region.size, page_size);
  EXPECT_EQ(info[1].region.size, 2 * page_size);

  // Query with larger buffer than available
  zx_iob_region_info_t large_info[10]{};
  ASSERT_OK(ep0.get_info(ZX_INFO_IOB_REGIONS, large_info, sizeof(large_info), &actual, &avail));
  EXPECT_EQ(actual, 3u);
  EXPECT_EQ(avail, 3u);

  // Calling ZX_INFO_IOB and ZX_INFO_IOB_REGIONS on non-IOB handle
  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));
  zx_info_iob_t iob_info{};
  EXPECT_EQ(ZX_ERR_WRONG_TYPE,
            ch0.get_info(ZX_INFO_IOB, &iob_info, sizeof(iob_info), nullptr, nullptr));
  EXPECT_EQ(ZX_ERR_WRONG_TYPE,
            ch0.get_info(ZX_INFO_IOB_REGIONS, info, sizeof(info), nullptr, nullptr));
}

}  // namespace
