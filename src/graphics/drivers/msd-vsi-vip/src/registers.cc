// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "registers.h"

#include "format"

namespace registers {

// We need to use a macro here to get the "RegName" as a string.
#define DUMP_REGISTER(RegName) \
  dump_out->emplace_back(std::format(#RegName " {:#x}", RegName::Get().ReadFrom(io).reg_value()));

void PrintRegisters(std::vector<std::string>* dump_out, magma::RegisterIo* io) {
  dump_out->emplace_back("---- Reg dump begin --- ");
  DUMP_REGISTER(ClockControl);
  DUMP_REGISTER(IrqAck);
  DUMP_REGISTER(IrqEnable);
  DUMP_REGISTER(ChipId);
  DUMP_REGISTER(Revision);
  DUMP_REGISTER(ChipDate);
  DUMP_REGISTER(ProductId);
  DUMP_REGISTER(EcoId);
  DUMP_REGISTER(CustomerId);
  DUMP_REGISTER(Features);
  DUMP_REGISTER(Specs1);
  DUMP_REGISTER(Specs2);
  DUMP_REGISTER(Specs3);
  DUMP_REGISTER(Specs4);
  DUMP_REGISTER(PowerModule);
  DUMP_REGISTER(PulseEater);
  DUMP_REGISTER(MmuConfig);
  DUMP_REGISTER(MmuPageTableArrayConfig);
  DUMP_REGISTER(IdleState);
  DUMP_REGISTER(MmuSecureExceptionAddress);
  DUMP_REGISTER(MmuSecureStatus);
  DUMP_REGISTER(MmuSecureControl);
  DUMP_REGISTER(PageTableArrayAddressLow);
  DUMP_REGISTER(PageTableArrayAddressHigh);
  DUMP_REGISTER(PageTableArrayControl);
  DUMP_REGISTER(MmuNonSecuritySafeAddressLow);
  DUMP_REGISTER(MmuSecuritySafeAddressLow);
  DUMP_REGISTER(MmuSafeAddressConfig);
  DUMP_REGISTER(SecureCommandControl);
  DUMP_REGISTER(SecureAhbControl);
  DUMP_REGISTER(FetchEngineCommandAddress);
  DUMP_REGISTER(FetchEngineCommandControl);
  DUMP_REGISTER(DmaStatus);
  DUMP_REGISTER(DmaDebugState);
  DUMP_REGISTER(DmaAddress);
  dump_out->emplace_back("---- Reg dump end --- ");
}

}  // namespace registers
