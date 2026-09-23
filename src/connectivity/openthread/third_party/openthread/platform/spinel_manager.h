/*
 *  Copyright (c) 2024, The OpenThread Authors.
 *  All rights reserved.
 *
 *  Redistribution and use in source and binary forms, with or without
 *  modification, are permitted provided that the following conditions are met:
 *  1. Redistributions of source code must retain the above copyright
 *     notice, this list of conditions and the following disclaimer.
 *  2. Redistributions in binary form must reproduce the above copyright
 *     notice, this list of conditions and the following disclaimer in the
 *     documentation and/or other materials provided with the distribution.
 *  3. Neither the name of the copyright holder nor the
 *     names of its contributors may be used to endorse or promote products
 *     derived from this software without specific prior written permission.
 *
 *  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
 *  AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 *  IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 *  ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
 *  LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
 *  CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
 *  SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
 *  INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
 *  CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
 *  ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
 *  POSSIBILITY OF SUCH DAMAGE.
 */

#ifndef SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_SPINEL_MANAGER_H_
#define SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_SPINEL_MANAGER_H_

#include <assert.h>

#include "radio_url.h"
#include "spinel_fidl_interface.h"

#include <common/code_utils.hpp>
#include <spinel/spinel_driver.hpp>
#define OPENTHREAD_POSIX_CONFIG_SPINEL_VENDOR_INTERFACE_ENABLE 1

namespace ot {
namespace Posix {
typedef ot::Fuchsia::SpinelFidlInterface VendorInterface;

class SpinelManager {
 public:
  /**
   * Returns the static instance of the SpinelManager.
   */
  static SpinelManager &GetSpinelManager(void);

  /**
   * Returns the static instance of the SpinelDriver.
   */
  Spinel::SpinelDriver &GetSpinelDriver(void) { return mSpinelDriver; }

  /**
   * Constructor of the SpinelManager
   */
  SpinelManager(void);

  /**
   * Destructor of the SpinelManager
   */
  ~SpinelManager(void);

  /**
   * Initializes the SpinelManager.
   *
   * @param[in]   aUrl  A pointer to the null-terminated spinel URL.
   *
   * @retval  OT_COPROCESSOR_UNKNOWN  The initialization fails.
   * @retval  OT_COPROCESSOR_RCP      The Co-processor is a RCP.
   * @retval  OT_COPROCESSOR_NCP      The Co-processor is a NCP.
   */
  CoprocessorType Init(const char *aUrl);

  /**
   * Deinitializes the SpinelManager.
   */
  void Deinit(void);

  /**
   * Returns the spinel interface.
   *
   * @returns The spinel interface.
   */
  Spinel::SpinelInterface &GetSpinelInterface(void) {
    assert(mSpinelInterface != nullptr);
    return *mSpinelInterface;
  }

 private:
#if OPENTHREAD_POSIX_VIRTUAL_TIME
  void VirtualTimeInit(void);
#endif
  void GetIidListFromUrl(spinel_iid_t (&aIidList)[Spinel::kSpinelHeaderMaxNumIid]);

  Spinel::SpinelInterface *CreateSpinelInterface(const char *aInterfaceName);

#if OPENTHREAD_POSIX_CONFIG_SPINEL_HDLC_INTERFACE_ENABLE && \
    OPENTHREAD_POSIX_CONFIG_SPINEL_SPI_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(Posix::SpiInterface) >
                                                            sizeof(Posix::HdlcInterface)
                                                        ? sizeof(Posix::SpiInterface)
                                                        : sizeof(Posix::HdlcInterface);
#elif OPENTHREAD_POSIX_CONFIG_SPINEL_HDLC_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(Posix::HdlcInterface);
#elif OPENTHREAD_POSIX_CONFIG_SPINEL_SPI_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(Posix::SpiInterface);
#elif OPENTHREAD_POSIX_CONFIG_SPINEL_VENDOR_INTERFACE_ENABLE
  static constexpr size_t kSpinelInterfaceRawSize = sizeof(Posix::VendorInterface);
#else
#error "No Spinel interface is specified!"
#endif

  RadioUrl mUrl;
  Spinel::SpinelDriver mSpinelDriver;
  Spinel::SpinelInterface *mSpinelInterface;

  OT_DEFINE_ALIGNED_VAR(mSpinelInterfaceRaw, kSpinelInterfaceRawSize, uint64_t);
};

}  // namespace Posix
}  // namespace ot

CoprocessorType platformSpinelManagerInit(const char *aUrl);
extern "C" void platformSpinelManagerProcess(otInstance *aInstance,
                                             const otSysMainloopContext *aContext);

#endif  // SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_SPINEL_MANAGER_H_
