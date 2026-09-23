// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_SPINEL_FIDL_INTERFACE_H_
#define SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_SPINEL_FIDL_INTERFACE_H_

#include <stdint.h>
#include <string.h>

#include <openthread/error.h>
#include <spinel/spinel.h>

#include "openthread-system.h"
#include "radio_url.h"

#include <spinel/spinel_interface.hpp>

namespace ot {
namespace Fuchsia {

void spinelInterfaceInit(otInstance* a_instance);
/**
 * This class defines an spinel interface to the Radio Co-processor (RCP)
 *
 */
class SpinelFidlInterface : public ot::Spinel::SpinelInterface {
 public:
  /**
   * Constructor
   *
   */
  SpinelFidlInterface(const Url::Url& aRadioUrl);
  /**
   * Initializes the Spinel Fidl Interface
   *
   */
  otError Init(ReceiveFrameCallback aCallback, void* aCallbackContext, RxFrameBuffer& aFrameBuffer);
  /**
   * Deinitialized the instance
   *
   */

  void Deinit(void);
  /**
   * Send the frame from ot-lib to ot-radio driver
   *
   */
  otError SendFrame(const uint8_t* aFrame, uint16_t aLength);
  /**
   * Used for waiting for a spinel frame response with a timeout
   *
   */
  otError WaitForFrame(uint64_t aTimeoutUs);
  /**
   * Used for process a spinel frame event
   *
   */
  void Process(const void* aContext);

  /**
   * This method is called when RCP failure detected and resets internal states of the interface.
   *
   */
  void OnRcpReset(void);

  /**
   * This method is called when RCP is reset to recreate the connection with it.
   *
   */
  otError ResetConnection(void);

  /**
   * This method hardware resets the RCP.
   *
   * @retval OT_ERROR_NONE            Successfully reset the RCP.
   * @retval OT_ERROR_NOT_IMPLEMENT   The hardware reset is not implemented.
   *
   */
  otError HardwareReset(void) { return OT_ERROR_NOT_IMPLEMENTED; }

  /**
   * Returns the RCP interface metrics.
   *
   * @returns The RCP interface metrics.
   *
   */
  const otRcpInterfaceMetrics* GetRcpInterfaceMetrics(void) const { return &mInterfaceMetrics; }

  void UpdateFdSet(void* aMainloopContext) {}

  uint32_t GetBusSpeed(void) const { return 0; }

  static bool IsInterfaceNameMatch(const char* aInterfaceName) { return true; }

 private:
  /**
   * Write received inbound frame to the buffer where can be processed by ot-lib
   *
   */
  void WriteToRxFrameBuffer(std::vector<uint8_t> vec);
  ReceiveFrameCallback mReceiveFrameCallback;
  void* mReceiveFrameContext;
  RxFrameBuffer* mRxFrameBuffer;

  otRcpInterfaceMetrics mInterfaceMetrics;
  [[maybe_unused]]
  const Url::Url& mRadioUrl;
};  // SpinelFidlInterface
}  // namespace Fuchsia
}  // namespace ot

#endif  // SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_SPINEL_FIDL_INTERFACE_H_
