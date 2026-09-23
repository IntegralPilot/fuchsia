// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/intl_provider.h"

#include <fidl/fuchsia.intl/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/zx/time.h>

#include <utility>
#include <vector>

namespace forensics::stubs {
namespace {

using fuchsia_intl::LocaleId;
using fuchsia_intl::Profile;
using fuchsia_intl::TimeZoneId;

Profile MakeProfile(const std::optional<std::string>& locale,
                    const std::optional<std::string>& timezone) {
  Profile profile;
  if (locale.has_value()) {
    profile.locales(std::vector{
        LocaleId{{
            .id = *locale,
        }},
    });
  }

  if (timezone.has_value()) {
    profile.time_zones(std::vector{
        TimeZoneId{{
            .id = *timezone,
        }},
    });
  }

  return profile;
}

}  // namespace

IntlProvider::IntlProvider(std::optional<std::string> default_locale,
                           std::optional<std::string> default_timezone)
    : locale_(std::move(default_locale)), timezone_(std::move(default_timezone)) {}

void IntlProvider::GetProfile(GetProfileCompleter::Sync& completer) {
  completer.Reply(MakeProfile(locale_, timezone_));
}

void IntlProvider::SetLocale(std::string_view locale) {
  locale_ = std::string(locale);
  if (!binding().has_value() || !IsBound()) {
    return;
  }

  std::ignore = fidl::SendEvent(binding().value())->OnChange();
}

void IntlProvider::SetTimezone(std::string_view timezone) {
  timezone_ = std::string(timezone);
  if (!binding().has_value() || !IsBound()) {
    return;
  }

  std::ignore = fidl::SendEvent(binding().value())->OnChange();
}

IntlProviderDelaysResponse::IntlProviderDelaysResponse(async_dispatcher_t* dispatcher,
                                                       zx::duration delay,
                                                       std::optional<std::string> default_locale,
                                                       std::optional<std::string> default_timezone)
    : dispatcher_(dispatcher),
      delay_(delay),
      locale_(std::move(default_locale)),
      timezone_(std::move(default_timezone)) {}

void IntlProviderDelaysResponse::GetProfile(GetProfileCompleter::Sync& completer) {
  async::PostDelayedTask(
      dispatcher_,
      [locale = locale_, timezone = timezone_, completer = completer.ToAsync()]() mutable {
        completer.Reply(MakeProfile(locale, timezone));
      },
      delay_);
}

}  // namespace forensics::stubs
