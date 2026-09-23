// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/trace-provider/provider.h>

#include <string>
#include <utility>

#include <zxtest/zxtest.h>

#include "lib/fidl/cpp/wire/channel.h"
#include "lib/fit/internal/result.h"
#include "zxtest/base/parameterized-value.h"
#include "zxtest/base/values.h"

namespace trace {
namespace {

class FakeTraceManager : public fidl::Server<fuchsia_tracing_provider::Registry> {
 public:
  FakeTraceManager(async_dispatcher_t* dispatcher,
                   fidl::ServerEnd<fuchsia_tracing_provider::Registry> server_end,
                   std::vector<std::string> categories,
                   fuchsia_tracing::BufferingMode buffering_mode)
      : categories_(std::move(categories)),
        buffering_mode_(buffering_mode),
        dispatcher_(dispatcher) {
    fidl::BindServer(dispatcher_, std::move(server_end), this,
                     [](FakeTraceManager* impl, fidl::UnbindInfo info,
                        fidl::ServerEnd<fuchsia_tracing_provider::Registry> server_end) {
                       fprintf(stderr, "FakeTraceManager: FIDL server unbound: info=%s\n",
                               info.FormatDescription().c_str());
                     });
  }

  void RegisterProvider(fuchsia_tracing_provider::RegistryRegisterProviderRequest& request,
                        RegisterProviderCompleter::Sync& completer) override {}

  void RegisterProviderSynchronously(
      fuchsia_tracing_provider::RegistryRegisterProviderSynchronouslyRequest& request,
      RegisterProviderSynchronouslyCompleter::Sync& completer) override {}

  void RegisterV2(RegisterV2Request& request, RegisterV2Completer::Sync& completer) override {
    provider_client_.emplace(std::move(request.provider()), dispatcher_);

    zx::vmo buffer_vmo;
    ASSERT_EQ(zx::vmo::create(42, 0u, &buffer_vmo), ZX_OK);

    fuchsia_tracing_provider::ProviderConfigV2 config{{
        .buffering_mode = buffering_mode_,
        .buffer = std::move(buffer_vmo),
        .categories = categories_,
    }};

    fit::result result = provider_client_.value()->Initialize({std::move(config)});
    ASSERT_TRUE(result.is_ok(), "%s error calling Initialize: %s", request.name().c_str(),
                result.error_value().status_string());
  }

  void RegisterV2Synchronously(RegisterV2SynchronouslyRequest& request,
                               RegisterV2SynchronouslyCompleter::Sync& completer) override {
    completer.Reply(fit::ok(fuchsia_tracing_provider::RegistryRegisterV2SynchronouslyResponse{{
        .started = false,
    }}));
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_provider::Registry> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  std::optional<fidl::Client<fuchsia_tracing_provider::ProviderV2>>& provider_client() {
    return provider_client_;
  }

 private:
  const std::vector<std::string> categories_;
  const fuchsia_tracing::BufferingMode buffering_mode_;
  std::optional<fidl::Client<fuchsia_tracing_provider::ProviderV2>> provider_client_;
  async_dispatcher_t* dispatcher_;
};

class ProviderTestBase : public zxtest::Test {
 public:
  ProviderTestBase(const std::vector<std::string>& categories,
                   fuchsia_tracing::BufferingMode buffering_mode)
      : endpoints_(fidl::CreateEndpoints<fuchsia_tracing_provider::Registry>()) {
    ASSERT_TRUE(endpoints_.is_ok(), "%s", endpoints_.status_string());

    manager_.emplace(loop_.dispatcher(), std::move(endpoints_->server), categories, buffering_mode);
    provider_.emplace(endpoints_->client.TakeChannel(), loop_.dispatcher());
  }

  void TearDown() {
    // `provider_` must be deleted before `manager_` to avoid use after free. Changing declaration
    // order is insufficient.
    provider_.reset();
    manager_.reset();
    loop_.Shutdown();
  }

 protected:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::optional<FakeTraceManager> manager_;
  std::optional<TraceProvider> provider_;
  zx::result<fidl::Endpoints<fuchsia_tracing_provider::Registry>> endpoints_;
};

class ProviderTest : public ProviderTestBase {
 public:
  ProviderTest() : ProviderTestBase({}, fuchsia_tracing::BufferingMode::kOneshot) {}
};

// Test handling of early loop cancel by having the loop be destructed before the provider.
TEST_F(ProviderTest, EarlyLoopCancel) {
  loop_.RunUntilIdle();
  loop_.Shutdown();
}

TEST_F(ProviderTest, GetKnownCategoriesDefault) {
  loop_.RunUntilIdle();

  ASSERT_TRUE(manager_->provider_client().has_value());
  manager_->provider_client().value()->GetKnownCategories().Then(
      [](fidl::Result<::fuchsia_tracing_provider::ProviderV2::GetKnownCategories>& result) {
        ASSERT_TRUE(result.is_ok());
        // The default known category list will be empty until static compile-time registration is
        // implemented.
        ASSERT_EQ(0, result->categories().size());
      });
  // Wait for the fidl callbacks to complete.
  loop_.RunUntilIdle();
}

TEST_F(ProviderTest, GetKnownCategoriesFromSetCallback) {
  loop_.RunUntilIdle();

  provider_->SetGetKnownCategoriesCallback([]() {
    return std::vector<trace::KnownCategory>{
        {.name = "foo"},
        {.name = "bar"},
        {.name = "baz", .description = "description"},
    };
  });

  ASSERT_TRUE(manager_->provider_client().has_value());
  manager_->provider_client().value()->GetKnownCategories().Then(
      [](fidl::Result<::fuchsia_tracing_provider::ProviderV2::GetKnownCategories>& result) {
        ASSERT_TRUE(result.is_ok());
        std::vector<fuchsia_tracing::KnownCategory> expected_known_categories = {
            {{.name = "foo"}},
            {{.name = "bar"}},
            {{.name = "baz", .description = "description"}},
        };
        ASSERT_EQ(expected_known_categories, result->categories());
      });
  // Wait for the fidl callbacks to complete.
  loop_.RunUntilIdle();
}

TEST_F(ProviderTest, DoubleInitialize) {
  loop_.RunUntilIdle();

  ASSERT_TRUE(manager_->provider_client().has_value());

  // The first Initialize has already been called during RegisterV2.
  // Now call it again.
  zx::vmo buffer_vmo;
  ASSERT_EQ(zx::vmo::create(42, 0u, &buffer_vmo), ZX_OK);
  fuchsia_tracing_provider::ProviderConfigV2 config{{
      .buffering_mode = fuchsia_tracing::BufferingMode::kOneshot,
      .buffer = std::move(buffer_vmo),
  }};

  auto result = (*manager_->provider_client())->Initialize({std::move(config)});
  ASSERT_TRUE(result.is_ok());

  // Since it closes the channel on error, we should expect the channel to be closed.
  loop_.RunUntilIdle();

  bool called = false;
  (*manager_->provider_client())
      ->GetKnownCategories()
      .Then([&called](
                fidl::Result<::fuchsia_tracing_provider::ProviderV2::GetKnownCategories>& result) {
        called = true;
        ASSERT_TRUE(result.is_error());
        // The channel should be closed with the epitaph ZX_ERR_BAD_STATE.
        EXPECT_EQ(result.error_value().status(), ZX_ERR_BAD_STATE);
      });

  loop_.RunUntilIdle();
  ASSERT_TRUE(called);
}

struct TestParams {
  std::vector<std::string> categories;
  fuchsia_tracing::BufferingMode buffering_mode;
  ProviderConfig expected_config;
};

class ParameterizedProviderTest : public ProviderTestBase,
                                  public zxtest::WithParamInterface<TestParams> {
 public:
  ParameterizedProviderTest()
      : ProviderTestBase(GetParam().categories, GetParam().buffering_mode) {}
};

// Test that the provider config sent to the provider on initialization is made available via
// GetProviderConfig.
TEST_P(ParameterizedProviderTest, GetProviderConfig) {
  loop_.RunUntilIdle();

  EXPECT_EQ(GetParam().expected_config.categories, provider_->GetProviderConfig().categories);
  EXPECT_EQ(GetParam().expected_config.buffering_mode,
            provider_->GetProviderConfig().buffering_mode);
}

INSTANTIATE_TEST_SUITE_P(
    ProviderTest, ParameterizedProviderTest,
    zxtest::Values(TestParams{.categories = {"expirationsun", "crossfoil"},
                              .buffering_mode = fuchsia_tracing::BufferingMode::kOneshot,
                              .expected_config =
                                  {
                                      .buffering_mode = TRACE_BUFFERING_MODE_ONESHOT,
                                      .categories = {"expirationsun", "crossfoil"},
                                  }},
                   TestParams{.buffering_mode = fuchsia_tracing::BufferingMode::kCircular,
                              .expected_config =
                                  {
                                      .buffering_mode = TRACE_BUFFERING_MODE_CIRCULAR,
                                  }},
                   TestParams{.buffering_mode = fuchsia_tracing::BufferingMode::kStreaming,
                              .expected_config = {
                                  .buffering_mode = TRACE_BUFFERING_MODE_STREAMING,
                              }}));

}  // namespace
}  // namespace trace
