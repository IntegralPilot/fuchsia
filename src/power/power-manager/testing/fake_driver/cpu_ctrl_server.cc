// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpu_ctrl_server.h"

#include <lib/driver/logging/cpp/structured_logger.h>

namespace fake_driver {
CpuCtrlProtocolServer::CpuCtrlProtocolServer() {}

void CpuCtrlProtocolServer::GetOperatingPointInfo(GetOperatingPointInfoRequestView request,
                                                  GetOperatingPointInfoCompleter::Sync& completer) {
  if (request->opp >= kOperatingPoints.size()) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  fuchsia_hardware_cpu_ctrl::wire::CpuOperatingPointInfo result;
  result.frequency_hz = kOperatingPoints[request->opp].freq_hz;
  result.voltage_uv = kOperatingPoints[request->opp].volt_uv;

  completer.ReplySuccess(result);
}

void CpuCtrlProtocolServer::SetCurrentOperatingPoint(
    SetCurrentOperatingPointRequestView request,
    SetCurrentOperatingPointCompleter::Sync& completer) {
  std::scoped_lock lock(lock_);
  current_opp_ = request->requested_opp;
  completer.ReplySuccess(current_opp_);
}

void CpuCtrlProtocolServer::GetCurrentOperatingPoint(
    GetCurrentOperatingPointCompleter::Sync& completer) {
  std::scoped_lock lock(lock_);
  completer.Reply(current_opp_);
}

void CpuCtrlProtocolServer::SetMinimumOperatingPointLimit(
    SetMinimumOperatingPointLimitRequestView request,
    SetMinimumOperatingPointLimitCompleter::Sync& completer) {
  if (request->minimum_opp >= kOperatingPoints.size()) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  {
    std::scoped_lock lock(lock_);
    minimum_opp_ = request->minimum_opp;
  }

  completer.ReplySuccess();
}

void CpuCtrlProtocolServer::SetMaximumOperatingPointLimit(
    SetMaximumOperatingPointLimitRequestView request,
    SetMaximumOperatingPointLimitCompleter::Sync& completer) {
  if (request->maximum_opp >= kOperatingPoints.size()) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  {
    std::scoped_lock lock(lock_);
    maximum_opp_ = request->maximum_opp;
  }

  completer.ReplySuccess();
}

void CpuCtrlProtocolServer::SetOperatingPointLimits(
    SetOperatingPointLimitsRequestView request, SetOperatingPointLimitsCompleter::Sync& completer) {
  if (request->minimum_opp >= kOperatingPoints.size() ||
      request->maximum_opp >= kOperatingPoints.size()) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  {
    std::scoped_lock lock(lock_);
    minimum_opp_ = request->minimum_opp;
    maximum_opp_ = request->maximum_opp;
  }

  completer.ReplySuccess();
}

void CpuCtrlProtocolServer::GetCurrentOperatingPointLimits(
    GetCurrentOperatingPointLimitsCompleter::Sync& completer) {
  std::scoped_lock lock(lock_);
  completer.ReplySuccess(minimum_opp_, maximum_opp_);
}

void CpuCtrlProtocolServer::GetOperatingPointCount(
    GetOperatingPointCountCompleter::Sync& completer) {
  completer.ReplySuccess(static_cast<uint32_t>(kOperatingPoints.size()));
}

void CpuCtrlProtocolServer::GetNumLogicalCores(GetNumLogicalCoresCompleter::Sync& completer) {
  completer.Reply(static_cast<uint64_t>(kLogicalCoreIds.size()));
}

void CpuCtrlProtocolServer::GetLogicalCoreId(GetLogicalCoreIdRequestView request,
                                             GetLogicalCoreIdCompleter::Sync& completer) {
  if (request->index >= kLogicalCoreIds.size()) {
    completer.Close(ZX_ERR_OUT_OF_RANGE);
  }
  completer.Reply(kLogicalCoreIds[request->index]);
}

void CpuCtrlProtocolServer::GetDomainId(GetDomainIdCompleter::Sync& completer) {
  completer.Reply(0);
}

void CpuCtrlProtocolServer::GetRelativePerformance(
    GetRelativePerformanceCompleter::Sync& completer) {
  completer.ReplySuccess(255);
}

void CpuCtrlProtocolServer::GetRelativePerformance2(
    GetRelativePerformance2Completer::Sync& completer) {
  completer.ReplySuccess(255);
}

void CpuCtrlProtocolServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_cpu_ctrl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FDF_SLOG(ERROR, "Unknown FIDL method ordinal", KV("ordinal", metadata.method_ordinal));
}

void CpuCtrlProtocolServer::Serve(async_dispatcher_t* dispatcher,
                                  fidl::ServerEnd<fuchsia_hardware_cpu_ctrl::Device> server) {
  bindings_.AddBinding(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace fake_driver
