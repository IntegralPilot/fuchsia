// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/default.h>
#include <lib/driver/fake-clock/cpp/fake-clock.h>

namespace fdf_fake {

FakeClock::FakeClock(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
  if (!dispatcher_) {
    dispatcher_ = async_get_default_dispatcher();
  }
}

void FakeClock::Enable(EnableCompleter::Sync& completer) {
  if (enable_result_.is_error()) {
    completer.ReplyError(enable_result_.status_value());
    return;
  }
  enabled_ = true;
  completer.ReplySuccess();
}

void FakeClock::Disable(DisableCompleter::Sync& completer) {
  if (disable_result_.is_error()) {
    completer.ReplyError(disable_result_.status_value());
    return;
  }
  enabled_ = false;
  completer.ReplySuccess();
}

void FakeClock::IsEnabled(IsEnabledCompleter::Sync& completer) {
  completer.ReplySuccess(enabled_.value_or(false));
}

void FakeClock::SetRate(SetRateRequestView request, SetRateCompleter::Sync& completer) {
  if (set_rate_result_.is_error()) {
    completer.ReplyError(set_rate_result_.status_value());
    return;
  }
  if (request->hz > max_rate_.value_or(std::numeric_limits<uint64_t>::max())) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  rate_ = request->hz;
  completer.ReplySuccess();
}

void FakeClock::QuerySupportedRate(QuerySupportedRateRequestView request,
                                   QuerySupportedRateCompleter::Sync& completer) {
  if (supported_rates_.empty()) {
    completer.ReplySuccess(request->hz_in);
    return;
  }

  uint64_t result_rate = 0;
  for (const uint64_t rate : supported_rates_) {
    if (rate <= request->hz_in) {
      result_rate = rate;
    }
  }

  if (result_rate == 0) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  } else {
    completer.ReplySuccess(result_rate);
  }
}

void FakeClock::GetRate(GetRateCompleter::Sync& completer) {
  if (get_rate_result_.has_value()) {
    if (get_rate_result_->is_error()) {
      completer.ReplyError(get_rate_result_->status_value());
      return;
    }
    completer.ReplySuccess(get_rate_result_->value());
    return;
  }
  completer.ReplySuccess(rate_.value_or(0));
}

void FakeClock::SetInput(SetInputRequestView request, SetInputCompleter::Sync& completer) {
  if (set_input_result_.is_error()) {
    completer.ReplyError(set_input_result_.status_value());
    return;
  }
  input_idx_ = request->idx;
  completer.ReplySuccess();
}

void FakeClock::GetNumInputs(GetNumInputsCompleter::Sync& completer) { completer.ReplySuccess(1); }

void FakeClock::GetInput(GetInputCompleter::Sync& completer) {
  completer.ReplySuccess(input_idx_.value_or(0));
}

void FakeClock::GetProperties(GetPropertiesCompleter::Sync& completer) {
  completer.Reply(id_, fidl::StringView::FromExternal(name_));
}

std::optional<bool> FakeClock::take_enabled() {
  std::optional<bool> res = enabled_;
  enabled_.reset();
  return res;
}

std::optional<uint64_t> FakeClock::take_rate() {
  std::optional<uint64_t> res = rate_;
  rate_.reset();
  return res;
}

std::optional<uint32_t> FakeClock::take_input_idx() {
  std::optional<uint32_t> res = input_idx_;
  input_idx_.reset();
  return res;
}

std::optional<uint64_t> FakeClock::take_max_rate() {
  std::optional<uint64_t> res = max_rate_;
  max_rate_.reset();
  return res;
}

void FakeClock::Bind(async_dispatcher_t* dispatcher,
                     fidl::ServerEnd<fuchsia_hardware_clock::Clock> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

fidl::ClientEnd<fuchsia_hardware_clock::Clock> FakeClock::Connect(async_dispatcher_t* dispatcher) {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_clock::Clock>::Create();
  Bind(dispatcher ? dispatcher : dispatcher_, std::move(endpoints.server));
  return std::move(endpoints.client);
}

fuchsia_hardware_clock::Service::InstanceHandler FakeClock::CreateInstanceHandler(
    async_dispatcher_t* dispatcher) {
  return fuchsia_hardware_clock::Service::InstanceHandler({
      .clock = bindings_.CreateHandler(this, dispatcher ? dispatcher : dispatcher_,
                                       fidl::kIgnoreBindingClosure),
  });
}

}  // namespace fdf_fake
