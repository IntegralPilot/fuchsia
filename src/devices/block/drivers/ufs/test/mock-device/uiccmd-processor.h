// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_UICCMD_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_UICCMD_PROCESSOR_H_

#include <functional>

#include "handler.h"
#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {
namespace ufs_mock_device {

class UfsMockDevice;

class UicCmdProcessor {
 public:
  using UicCmdHandler = std::function<void(UfsMockDevice &, uint32_t, uint32_t, uint32_t)>;

  UicCmdProcessor(const UicCmdProcessor &) = delete;
  UicCmdProcessor &operator=(const UicCmdProcessor &) = delete;
  UicCmdProcessor(const UicCmdProcessor &&) = delete;
  UicCmdProcessor &operator=(const UicCmdProcessor &&) = delete;
  ~UicCmdProcessor() = default;
  explicit UicCmdProcessor(UfsMockDevice &mock_device) : mock_device_(mock_device) {}

  void HandleUicCmd(UicCommandOpcode value);

  static void DefaultDmeGetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2,
                                   uint32_t ucmdarg3);
  static void DefaultDmeSetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2,
                                   uint32_t ucmdarg3);
  static void DefaultDmePeerGetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                       uint32_t ucmdarg2, uint32_t ucmdarg3);
  static void DefaultDmePeerSetHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                       uint32_t ucmdarg2, uint32_t ucmdarg3);
  static void DefaultDmeLinkStartUpHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                           uint32_t ucmdarg2, uint32_t ucmdarg3);
  static void DefaultDmeHibernateEnterHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                              uint32_t ucmdarg2, uint32_t ucmdarg3);
  static void DefaultDmeHibernateExitHandler(UfsMockDevice &mock_device, uint32_t ucmdarg1,
                                             uint32_t ucmdarg2, uint32_t ucmdarg3);

  DEF_DEFAULT_HANDLER_BEGIN(UicCommandOpcode, UicCmdHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmeGet, DefaultDmeGetHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmeSet, DefaultDmeSetHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmePeerGet, DefaultDmePeerGetHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmePeerSet, DefaultDmePeerSetHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmeLinkStartUp, DefaultDmeLinkStartUpHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmeHibernateEnter, DefaultDmeHibernateEnterHandler)
  DEF_DEFAULT_HANDLER(UicCommandOpcode::kDmeHibernateExit, DefaultDmeHibernateExitHandler)
  DEF_DEFAULT_HANDLER_END()

 private:
  UfsMockDevice &mock_device_;
};

}  // namespace ufs_mock_device
}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_UICCMD_PROCESSOR_H_
