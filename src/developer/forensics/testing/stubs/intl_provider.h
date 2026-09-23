// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_INTL_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_INTL_PROVIDER_H_

#include <fidl/fuchsia.intl/cpp/fidl.h>
#include <fidl/fuchsia.intl/cpp/test_base.h>

#include <optional>
#include <string>
#include <string_view>

#include "lib/zx/time.h"
#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics::stubs {

using IntlProviderBase = SingleBindingFidlServer<fuchsia_intl::PropertyProvider>;

class IntlProvider : public IntlProviderBase {
 public:
  explicit IntlProvider(std::optional<std::string> default_locale,
                        std::optional<std::string> default_timezone);

  void SetLocale(std::string_view locale);
  void SetTimezone(std::string_view timezone);

  // |fuchsia_intl::PropertyProvider|
  void GetProfile(GetProfileCompleter::Sync& completer) override;

 private:
  std::optional<std::string> locale_;
  std::optional<std::string> timezone_;
};

class IntlProviderDelaysResponse : public IntlProviderBase {
 public:
  IntlProviderDelaysResponse(async_dispatcher_t* dispatcher, zx::duration delay,
                             std::optional<std::string> default_locale,
                             std::optional<std::string> default_timezone);

  // |fuchsia_intl::PropertyProvider|
  void GetProfile(GetProfileCompleter::Sync& completer) override;

 private:
  async_dispatcher_t* dispatcher_;
  zx::duration delay_;
  std::optional<std::string> locale_;
  std::optional<std::string> timezone_;
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_INTL_PROVIDER_H_
