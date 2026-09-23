// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/efi/testing/fake_network_protocol.h>

#include <algorithm>
#include <iterator>
#include <span>

#include <efi/protocol/managed-network.h>

namespace efi {

efi_status FakeManagedNetworkProtocol::GetMode(efi_managed_network_config_data* mnp_data,
                                               efi_simple_network_mode* snp_data) const {
  if (mnp_data == nullptr || snp_data == nullptr) {
    return EFI_UNSUPPORTED;
  }

  *mnp_data = mnp_data_;
  *snp_data = snp_mode_data_;
  return EFI_SUCCESS;
}

efi_status FakeManagedNetworkProtocol::Configure(efi_managed_network_config_data* mnp_data) {
  return EFI_SUCCESS;
}

efi_status FakeManagedNetworkProtocol::Transmit(efi_managed_network_sync_completion_token* token) {
  if (!token || !token->Event || !token->Packet.TxData) {
    return EFI_UNSUPPORTED;
  }

  constexpr size_t kMacAddrSize = 6;

  efi_managed_network_transmit_data& tx_data = *token->Packet.TxData;
  most_recent_tx_.clear();

  const uint8_t* dst_ptr = reinterpret_cast<const uint8_t*>(tx_data.DestinationAddress);
  std::copy(dst_ptr, dst_ptr + kMacAddrSize, std::back_inserter(most_recent_tx_));

  const uint8_t* src_ptr = reinterpret_cast<const uint8_t*>(tx_data.SourceAddress);
  std::copy(src_ptr, src_ptr + kMacAddrSize, std::back_inserter(most_recent_tx_));

  // Quick and dirty htons
  const uint8_t* type_ptr = reinterpret_cast<const uint8_t*>(&(tx_data.ProtocolType));
  static_assert(sizeof(tx_data.ProtocolType) == 2);
  most_recent_tx_.push_back(type_ptr[1]);
  most_recent_tx_.push_back(type_ptr[0]);

  std::span<efi_managed_network_fragment_data> fragment_span(tx_data.FragmentTable,
                                                             tx_data.FragmentCount);
  for (const efi_managed_network_fragment_data& f : fragment_span) {
    const uint8_t* frag_ptr = reinterpret_cast<const uint8_t*>(f.FragmentBuffer);
    std::copy(frag_ptr, frag_ptr + f.FragmentLength, std::back_inserter(most_recent_tx_));
  }

  return EFI_SUCCESS;
}

}  // namespace efi
