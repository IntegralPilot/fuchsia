// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_FAKE_NETWORK_PROTOCOL_H_
#define ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_FAKE_NETWORK_PROTOCOL_H_

#include <vector>
#include <type_traits>

#include <efi/protocol/managed-network.h>

#include "efi/protocol/simple-network.h"

namespace efi {

class FakeManagedNetworkProtocol {
 public:
  explicit FakeManagedNetworkProtocol(efi_simple_network_mode data)
      : mnp_data_(), snp_mode_data_(data) {
    protocol_ = {
        .GetModeData = GetModeWrapper,
        .Configure = ConfigureWrapper,
        .Transmit = TransmitWrapper,
    };
  }

  FakeManagedNetworkProtocol(const FakeManagedNetworkProtocol&) = delete;
  FakeManagedNetworkProtocol& operator=(const FakeManagedNetworkProtocol&) = delete;

  efi_managed_network_protocol* protocol() { return &protocol_; }

  void SetModeData(const efi_simple_network_mode& mode) { snp_mode_data_ = mode; }
  void SetConfigData(const efi_managed_network_config_data& data) { mnp_data_ = data; }

  const std::vector<uint8_t>& GetMostRecentTx() const { return most_recent_tx_; }

 private:
  static efi_status GetModeWrapper(efi_managed_network_protocol* self,
                                   efi_managed_network_config_data* mnp_data,
                                   efi_simple_network_mode* snp_data) EFIAPI {
    return reinterpret_cast<FakeManagedNetworkProtocol*>(self)->GetMode(mnp_data, snp_data);
  }
  static efi_status ConfigureWrapper(efi_managed_network_protocol* self,
                                     efi_managed_network_config_data* mnp_data) EFIAPI {
    return reinterpret_cast<FakeManagedNetworkProtocol*>(self)->Configure(mnp_data);
  }
  static efi_status TransmitWrapper(efi_managed_network_protocol* self,
                                    efi_managed_network_sync_completion_token* token) EFIAPI {
    return reinterpret_cast<FakeManagedNetworkProtocol*>(self)->Transmit(token);
  }

  efi_status GetMode(efi_managed_network_config_data* mnp_data,
                     efi_simple_network_mode* snp_data) const;
  efi_status Configure(efi_managed_network_config_data* mnp_data);
  efi_status Transmit(efi_managed_network_sync_completion_token* token);

  efi_managed_network_protocol protocol_;
  efi_managed_network_config_data mnp_data_;
  efi_simple_network_mode snp_mode_data_;
  std::vector<uint8_t> most_recent_tx_;
};

// If this fails, we either need to modify our class layout or change how we
// convert between efi_disk_io_protocol and FakeManagedNetworkProtocol.
static_assert(std::is_standard_layout_v<FakeManagedNetworkProtocol>,
              "FakeManagedNetworkProtocol is not standard layout");

}  // namespace efi

#endif  // ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_FAKE_NETWORK_PROTOCOL_H_
