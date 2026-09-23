// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "performance-domain.h"

#include <fidl/fuchsia.device/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <iostream>

namespace {
const std::string kDeviceSuffix = "device";
}  // namespace

zx::result<CpuPerformanceDomain> CpuPerformanceDomain::CreateFromPath(const std::string& path) {
  std::string device_protocol_path = path + "/" + kDeviceSuffix;

  zx::result cpu = component::Connect<cpuctrl::Device>(device_protocol_path);
  if (cpu.is_error()) {
    return cpu.take_error();
  }

  return zx::ok(CpuPerformanceDomain(std::move(cpu.value())));
}

std::pair<zx_status_t, uint32_t> CpuPerformanceDomain::GetOperatingPointCount() {
  auto resp = cpu_client_->GetOperatingPointCount();
  return std::make_pair(resp.status(), resp.status() == ZX_OK ? resp.value()->count : 0);
}

std::pair<zx_status_t, uint64_t> CpuPerformanceDomain::GetNumLogicalCores() {
  auto resp = cpu_client_->GetNumLogicalCores();
  return std::make_pair(resp.status(), resp.status() == ZX_OK ? resp.value().count : 0);
}

std::pair<zx_status_t, uint64_t> CpuPerformanceDomain::GetRelativePerformance() {
  auto resp = cpu_client_->GetRelativePerformance();
  return std::make_pair(resp.status(),
                        resp.status() == ZX_OK ? resp.value()->relative_performance : 0);
}

std::pair<zx_status_t, uint64_t> CpuPerformanceDomain::GetRelativePerformance2() {
  auto resp = cpu_client_->GetRelativePerformance2();
  if (resp.status() != ZX_OK) {
    return std::make_pair(resp.status(), 0);
  }
  if (resp->is_error()) {
    return std::make_pair(resp->error_value(), 0);
  }
  return std::make_pair(ZX_OK, resp.value()->relative_performance);
}

std::pair<zx_status_t, uint64_t> CpuPerformanceDomain::GetDomainId() {
  auto resp = cpu_client_->GetDomainId();
  return std::make_pair(resp.status(), resp.status() == ZX_OK ? resp.value().domain_id : 0);
}

std::tuple<zx_status_t, uint64_t, cpuctrl::wire::CpuOperatingPointInfo>
CpuPerformanceDomain::GetCurrentOperatingPoint() {
  constexpr cpuctrl::wire::CpuOperatingPointInfo kEmptyOpp = {
      .frequency_hz = cpuctrl::wire::kFrequencyUnknown,
      .voltage_uv = cpuctrl::wire::kVoltageUnknown,
  };
  auto resp = cpu_client_->GetCurrentOperatingPoint();

  if (resp.status() != ZX_OK) {
    return std::make_tuple(resp.status(), 0, kEmptyOpp);
  }

  const auto [status, opps] = GetOperatingPoints();
  if (status != ZX_OK) {
    return std::make_tuple(status, 0, kEmptyOpp);
  }

  uint64_t current_opp = resp.value().out_opp;

  cpuctrl::wire::CpuOperatingPointInfo opp_result = kEmptyOpp;
  if (current_opp >= opps.size()) {
    std::cerr << "No description for current opp." << std::endl;
  } else {
    opp_result = opps[current_opp];
  }

  return std::make_tuple(ZX_OK, current_opp, opp_result);
}

std::tuple<zx_status_t, const std::vector<cpuctrl::wire::CpuOperatingPointInfo>&>
CpuPerformanceDomain::GetOperatingPoints() {
  // If we've already fetched this in the past, there's no need to fetch again.
  if (!cached_opps_.empty()) {
    return std::make_tuple(ZX_OK, std::ref(cached_opps_));
  }

  auto opp_count_resp = cpu_client_->GetOperatingPointCount();
  if (opp_count_resp.status() != ZX_OK) {
    std::cerr << "GetOperatingPointCount failed with error: " << opp_count_resp.status()
              << std::endl;
    return std::make_tuple(opp_count_resp.status(), std::ref(cached_opps_));
  }

  if (opp_count_resp->is_error()) {
    std::cerr << "GetOperatingPointCount failed with error: " << opp_count_resp->error_value()
              << std::endl;
    return std::make_tuple(opp_count_resp->error_value(), std::ref(cached_opps_));
  }

  for (uint32_t i = 0; i < opp_count_resp->value()->count; i++) {
    auto resp = cpu_client_->GetOperatingPointInfo(i);

    if (resp.status() != ZX_OK || resp->is_error()) {
      continue;
    }

    cached_opps_.push_back(resp->value()->info);
  }

  return std::make_tuple(ZX_OK, std::ref(cached_opps_));
}

zx_status_t CpuPerformanceDomain::SetCurrentOperatingPoint(uint32_t new_opp) {
  auto result = cpu_client_->SetCurrentOperatingPoint(new_opp);

  if (result.status() != ZX_OK) {
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }

  if (result->value()->out_opp != new_opp) {
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx_status_t CpuPerformanceDomain::SetOperatingPointLimits(uint32_t min_opp, uint32_t max_opp) {
  auto result = cpu_client_->SetOperatingPointLimits(min_opp, max_opp);
  if (result.status() != ZX_OK) {
    return result.status();
  }

  if (result->is_error()) {
    return result->error_value();
  }

  return ZX_OK;
}

std::tuple<zx_status_t, uint32_t, uint32_t> CpuPerformanceDomain::GetCurrentOperatingPointLimits() {
  auto result = cpu_client_->GetCurrentOperatingPointLimits();
  if (result.status() != ZX_OK) {
    return {result.status(), 0, 0};
  }

  if (result->is_error()) {
    return {result->error_value(), 0, 0};
  }

  return {ZX_OK, result.value()->minimum_opp, result->value()->maximum_opp};
}
