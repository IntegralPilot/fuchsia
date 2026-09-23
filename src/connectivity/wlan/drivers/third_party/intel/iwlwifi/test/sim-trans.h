// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//
// Simulated firmware for iwlwifi.
//
// This class actually simulates a transport layer ops (just like a PCI-e bus).
//
// By the way, this class also holds a 'iwl_trans' instance, which contains 'op_mode' and 'mvm'
// after Init() is called.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_TEST_SIM_TRANS_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_TEST_SIM_TRANS_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>

#include <memory>

#include <wlan/drivers/log_instance.h>

#include "third_party/iwlwifi/platform/kernel.h"
#include "third_party/iwlwifi/platform/wlanphyimpl-device.h"
#include "third_party/iwlwifi/test/sim-mvm.h"

namespace async {

class Loop;

}  // namespace async

namespace wlan {
namespace iwlwifi {

class RcuManager;

// SimTransIwlwifiDriver to mimic PcieIwlwifiDriver.
class SimTransIwlwifiDriver : public ::wlan::iwlwifi::WlanPhyDevice {
 public:
  explicit SimTransIwlwifiDriver(iwl_trans* drvdata, async_dispatcher_t* dispatcher);
  ~SimTransIwlwifiDriver();

  iwl_trans* drvdata() override;
  const iwl_trans* drvdata() const override;

  zx_status_t AddWlansoftmacDevice(uint16_t iface_id, struct iwl_mvm_vif* mvmvif) override;

  zx_status_t RemoveWlansoftmacDevice(uint16_t iface_id) override;

  size_t DeviceCount();

 private:
  // Store the addresses of allocated mvmvifs, in real world, these pointers are stored in
  // corresponding WlanSoftmacDevice instances.
  struct iwl_mvm_vif* mvmvif_ptrs_[MAX_NUM_MVMVIF];
  size_t softmac_device_count_ = 0;
  iwl_trans* drvdata_ = nullptr;
  std::unique_ptr<fdf::Logger> logger_;
};

}  // namespace iwlwifi

namespace testing {

// The struct to store the internal state of the simulated firmware.
struct sim_trans_priv {
  SimMvm* fw;

  // The pointer pointing back to a Test case for mock functions.  This must be initialized before
  // mock functions are called.
  void* test;
};

static inline struct sim_trans_priv* IWL_TRANS_GET_SIM_TRANS(struct iwl_trans* trans) {
  return (struct sim_trans_priv*)trans->trans_specific;
}

class SimTransport : public SimMvm {
 public:
  explicit SimTransport();
  ~SimTransport();

  // This function must be called before starting using other functions.
  zx_status_t Init();

  // Fake set/load firmware process like mock-ssk.
  void SetFirmware(std::string firmware);
  zx_status_t LoadFirmware(const char* name, zx_handle_t* vmo, size_t* size);

  // Member accessors.
  struct iwl_trans* iwl_trans();
  const struct iwl_trans* iwl_trans() const;
  ::wlan::iwlwifi::SimTransIwlwifiDriver* sim_driver();
  const ::wlan::iwlwifi::SimTransIwlwifiDriver* sim_driver() const;
  async_dispatcher_t* async_driver_dispatcher();
  fdf_dispatcher_t* fdf_driver_dispatcher();
  // This function starts a new managed dispatcher on demand of tests.
  fdf::UnownedSynchronizedDispatcher get_unowned_synchronized_dispatcher();

 private:
  // DriverRuntime maintains the foreground dispatcher and spawns the background dispatcher.
  fdf_testing::DriverRuntime runtime_;
  fdf::UnownedSynchronizedDispatcher sim_driver_dispatcher_ = runtime_.StartBackgroundDispatcher();

  std::unique_ptr<::async::Loop> task_loop_;
  std::unique_ptr<::async::Loop> irq_loop_;
  std::unique_ptr<::wlan::iwlwifi::RcuManager> rcu_manager_;
  struct device device_;
  struct iwl_trans* iwl_trans_;
  std::unique_ptr<wlan::iwlwifi::SimTransIwlwifiDriver> sim_driver_;
  std::vector<uint8_t> fake_firmware_;
};

}  // namespace testing
}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_TEST_SIM_TRANS_H_
