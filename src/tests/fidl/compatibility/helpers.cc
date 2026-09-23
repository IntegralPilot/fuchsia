// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.test.compatibility/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <utility>

#include <src/tests/fidl/compatibility/helpers.h>

using namespace component_testing;

namespace fidl_test_compatibility_helpers {

AllowImplPair Exclude(std::initializer_list<const char*> substrings) {
  return [substrings](const std::string& proxy_url, const std::string& server_url) {
    return std::all_of(std::begin(substrings), std::end(substrings), [&](const char* substring) {
      if (proxy_url.find(substring) != std::string::npos) {
        return false;
      }
      if (server_url.find(substring) != std::string::npos) {
        return false;
      }
      return true;
    });
  };
}

std::string ExtractShortName(const std::string& pkg_url) {
  std::string short_name;
  re2::RE2::PartialMatch(pkg_url, "(fidl-compatibility-test-)(.*)(#meta/impl\\.cm)", nullptr,
                         &short_name);
  return short_name;
}

void ForAllImpls(const Impls& impls, const TestBody& body) {
  ForSomeImpls(
      impls, [](const std::string& p, const std::string& s) { return true; }, body);
}

void ForSomeImpls(const Impls& impls, const AllowImplPair& allow, const TestBody& body) {
  for (auto const& proxy_url : impls) {
    for (auto const& server_url : impls) {
      if (!allow(proxy_url, server_url)) {
        continue;
      }
      bool test_completed = false;
      const std::string& proxy_short = ExtractShortName(proxy_url);
      const std::string& server_short = ExtractShortName(server_url);
      const std::string proxy_component = proxy_short + "_proxy";
      const std::string server_component = server_short + "_server";
      std::cerr << "Executing test for: " << proxy_short << " <-> " << server_short << std::endl;

      auto builder = RealmBuilder::Create();
      builder.AddChild(proxy_component, proxy_url,
                       ChildOptions{.startup_mode = StartupMode::EAGER});
      builder.AddChild(server_component, server_url,
                       ChildOptions{.startup_mode = StartupMode::EAGER});
      builder.AddRoute(Route{.capabilities = {Protocol{"fidl.test.compatibility.Echo"}},
                             .source = ChildRef{server_component},
                             .targets = {ChildRef{proxy_component}}});
      builder.AddRoute(Route{.capabilities = {Protocol{"fidl.test.compatibility.Echo"}},
                             .source = ChildRef{proxy_component},
                             .targets = {ParentRef()}});
      builder.AddRoute(Route{.capabilities = {Protocol{"fuchsia.logger.LogSink"},
                                              Protocol{"fuchsia.kernel.VmexResource"}},
                             .source = ParentRef(),
                             .targets = {ChildRef{server_component}, ChildRef{proxy_component}}});

      async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
      auto realm = builder.Build(loop.dispatcher());
      auto echo = realm.component().Connect<fidl::test::compatibility::Echo>();
      echo.set_error_handler([&proxy_url, &loop, &test_completed](zx_status_t status) {
        if (!test_completed) {
          loop.Quit();
          FAIL() << "Connection to " << proxy_url
                 << " failed unexpectedly: " << zx_status_get_string(status);
        }
      });

      body(loop, echo, server_url, proxy_url);
      test_completed = true;
    }
  }
}

bool GetImplsUnderTest(Impls* out_impls) {
  // TODO(https://fxbug.dev/42077626): this should come from structured config.
  zx::result client_end = component::Connect<fidl_test_compatibility::Config>();
  FX_CHECK(client_end.is_ok());
  fidl::SyncClient config(std::move(*client_end));
  auto result = config->GetImpls();
  FX_CHECK(result.is_ok()) << "Failed to get impls: " << result.error_value().status_string();
  auto impls = result->impls();

  for (size_t i = 0; i < impls.size(); i++) {
    out_impls->push_back(impls[i]);
  }

  if (!out_impls->empty()) {
    return true;
  }
  FX_CHECK(!out_impls->empty());
  return false;
}

zx::handle Handle() {
  zx_handle_t raw_event;
  const zx_status_t status = zx_event_create(0u, &raw_event);
  ZX_ASSERT_MSG(status == ZX_OK, "status = %s", zx_status_get_string(status));
  return zx::handle(raw_event);
}

::testing::AssertionResult HandlesEq(const zx::object_base& a, const zx::object_base& b) {
  if (a.is_valid() != b.is_valid()) {
    return ::testing::AssertionFailure()
           << "Handles are not equally valid :" << a.is_valid() << " vs " << b.is_valid();
  }
  if (!a.is_valid()) {
    return ::testing::AssertionSuccess() << "Both handles invalid";
  }
  zx_info_handle_basic_t a_info, b_info;
  if (zx_status_t status = zx_object_get_info(a.get(), ZX_INFO_HANDLE_BASIC, &a_info,
                                              sizeof(a_info), nullptr, nullptr);
      ZX_OK != status) {
    return ::testing::AssertionFailure()
           << "zx_object_get_info(a) returned " << zx_status_get_string(status);
  }
  if (zx_status_t status = zx_object_get_info(b.get(), ZX_INFO_HANDLE_BASIC, &b_info,
                                              sizeof(b_info), nullptr, nullptr);
      ZX_OK != status) {
    return ::testing::AssertionFailure()
           << "zx_object_get_info(b) returned " << zx_status_get_string(status);
  }
  if (a_info.koid != b_info.koid) {
    return ::testing::AssertionFailure() << std::endl
                                         << "a_info.koid is: " << a_info.koid << std::endl
                                         << "b_info.koid is: " << b_info.koid;
  }
  return ::testing::AssertionSuccess();
}

void PrintSummary(const Summary& summary) {
  std::cout << std::endl;
  std::cout << "========================= Interop Summary ======================" << std::endl;

  for (std::pair<std::string, bool> element : summary) {
    if (element.second) {
      std::cout << "[PASS]";
    } else {
      std::cout << "[FAIL]";
    }
    std::cout << " " << element.first << std::endl;
  }

  std::cout << std::endl;
  std::cout << std::endl;
}

std::string RandomUTF8(size_t count, std::default_random_engine& rand_engine) {
  std::uniform_int_distribution<uint32_t> uint32_distribution;
  std::string random_string;
  random_string.reserve(count);
  do {
    // Generate a random 32 bit unsigned int to use a the code point.
    uint32_t code_point = uint32_distribution(rand_engine);
    // Mask the random number so that it can be encoded into the number of bytes
    // remaining.
    size_t remaining = count - random_string.size();
    if (remaining == 1) {
      code_point &= 0x7F;
    } else if (remaining == 2) {
      code_point &= 0x7FF;
    } else if (remaining == 3) {
      code_point &= 0xFFFF;
    } else {
      // Mask to fall within the general range of code points.
      code_point &= 0x1FFFFF;
    }
    // Check that it's really a valid code point, otherwise try again.
    if (!fxl::IsValidCodepoint(code_point)) {
      continue;
    }
    // Add the character to the random string.
    fxl::WriteUnicodeCharacter(code_point, &random_string);
    FX_CHECK(random_string.size() <= count);
  } while (random_string.size() < count);
  return random_string;
}

}  // namespace fidl_test_compatibility_helpers
