// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/**
 * @file
 *   This file include declarations of functions that will be called in fuchsia component.
 */

#ifndef SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_RADIO_H_
#define SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_RADIO_H_

#define OPENTHREAD_POSIX_CONFIG_SPINEL_VENDOR_INTERFACE_ENABLE 1

#include "logger.h"
#include "openthread-system.h"
#include "radio_url.h"
#include "spinel_fidl_interface.h"
#include "spinel_manager.h"

#define OT_CORE_COMMON_NEW_HPP_  // Use the new operator defined in fuchsia instead
#include <spinel/radio_spinel.hpp>
#undef OT_CORE_COMMON_NEW_HPP_

extern "C" otError otPlatRadioEnable(otInstance *a_instance);
extern "C" otInstance *otPlatRadioCheckOtInstanceEnabled();
extern "C" void platformRadioProcess(otInstance *aInstance, const otSysMainloopContext *aContext);

namespace ot {
namespace Posix {
typedef ot::Fuchsia::SpinelFidlInterface VendorInterface;

/**
 * Manages Thread radio.
 */
class Radio : public Logger<Radio> {
 public:
  static const char kLogModuleName[];  ///< Module name used for logging.

  /**
   * Creates the radio manager.
   */
  Radio(void);

  /**
   * Initialize the Thread radio.
   *
   * @param[in]   aUrl    A pointer to the null-terminated URL.
   */
  void Init(const char *aUrl);

  /**
   * De-initializes the Thread radio.
   */
  void Deinit(void);

  /**
   * Acts as an accessor to the spinel interface instance used by the radio.
   *
   * @returns A reference to the radio's spinel interface instance.
   */
  Spinel::SpinelInterface &GetSpinelInterface(void) {
    return SpinelManager::GetSpinelManager().GetSpinelInterface();
  }

  /**
   * Acts as an accessor to the radio spinel instance used by the radio.
   *
   * @returns A reference to the radio spinel instance.
   */
  Spinel::RadioSpinel &GetRadioSpinel(void) { return mRadioSpinel; }

  /**
   * Acts as an accessor to the RCP capability diagnostic instance used by the radio.
   *
   * @returns A reference to the RCP capability diagnostic instance.
   */
#if OPENTHREAD_POSIX_CONFIG_RCP_CAPS_DIAG_ENABLE
  RcpCapsDiag &GetRcpCapsDiag(void) { return mRcpCapsDiag; }
#endif

 private:
  void ProcessRadioUrl(const RadioUrl &aRadioUrl);
  void ProcessMaxPowerTable(const RadioUrl &aRadioUrl);

#if OPENTHREAD_POSIX_CONFIG_TMP_STORAGE_ENABLE
  static void SaveRadioSpinelMetrics(const otRadioSpinelMetrics &aMetrics, void *aContext) {
    reinterpret_cast<Radio *>(aContext)->SaveRadioSpinelMetrics(aMetrics);
  }

  void SaveRadioSpinelMetrics(const otRadioSpinelMetrics &aMetrics) {
    mTmpStorage.SaveRadioSpinelMetrics(aMetrics);
  }

  static otError RestoreRadioSpinelMetrics(otRadioSpinelMetrics &aMetrics, void *aContext) {
    return reinterpret_cast<Radio *>(aContext)->RestoreRadioSpinelMetrics(aMetrics);
  }

  otError RestoreRadioSpinelMetrics(otRadioSpinelMetrics &aMetrics) {
    return mTmpStorage.RestoreRadioSpinelMetrics(aMetrics);
  }
#endif

#if OPENTHREAD_POSIX_CONFIG_SPINEL_HDLC_INTERFACE_ENABLE && \
    OPENTHREAD_POSIX_CONFIG_SPINEL_SPI_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(ot::Posix::SpiInterface) >
                                                            sizeof(ot::Posix::HdlcInterface)
                                                        ? sizeof(ot::Posix::SpiInterface)
                                                        : sizeof(ot::Posix::HdlcInterface);
#elif OPENTHREAD_POSIX_CONFIG_SPINEL_HDLC_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(ot::Posix::HdlcInterface);
#elif OPENTHREAD_POSIX_CONFIG_SPINEL_SPI_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(ot::Posix::SpiInterface);
#elif OPENTHREAD_POSIX_CONFIG_SPINEL_VENDOR_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(ot::Posix::VendorInterface);
#else
#error "No Spinel interface is specified!"
#endif

  static constexpr otRadioCaps kRequiredRadioCaps =
#if OPENTHREAD_CONFIG_THREAD_VERSION >= OT_THREAD_VERSION_1_2
      OT_RADIO_CAPS_TRANSMIT_SEC | OT_RADIO_CAPS_TRANSMIT_TIMING |
#endif
      OT_RADIO_CAPS_ACK_TIMEOUT | OT_RADIO_CAPS_TRANSMIT_RETRIES | OT_RADIO_CAPS_CSMA_BACKOFF;

  RadioUrl mRadioUrl;
#if OPENTHREAD_SPINEL_CONFIG_VENDOR_HOOK_ENABLE
  Spinel::VendorRadioSpinel mRadioSpinel;
#else
  Spinel::RadioSpinel mRadioSpinel;
#endif

#if OPENTHREAD_POSIX_CONFIG_RCP_CAPS_DIAG_ENABLE
  RcpCapsDiag mRcpCapsDiag;
#endif

#if OPENTHREAD_POSIX_CONFIG_TMP_STORAGE_ENABLE
  TmpStorage mTmpStorage;
#endif
};

}  // namespace Posix
}  // namespace ot

#endif  // SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_RADIO_H_
