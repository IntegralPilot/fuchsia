// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/directory.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

namespace provider = fuchsia_tracing_provider;

namespace {
const char kProviderRegistryPath[] = "svc/fuchsia.tracing.provider.Registry";
}  // namespace

// Trace providers use fdio so test it.
TEST_F(TraceManagerTest, RegisterProviderWithFdio) {
  zx::channel h1, h2;
  ASSERT_EQ(zx::channel::create(0, &h1, &h2), ZX_OK);
  ASSERT_EQ(fdio_service_connect_at(context_provider().outgoing_directory_ptr().channel().get(),
                                    kProviderRegistryPath, h2.release()),
            ZX_OK);
  fidl::Client<provider::Registry> registry_ptr{fidl::ClientEnd<provider::Registry>{std::move(h1)},
                                                dispatcher()};

  zx::channel ph1a, ph1b;
  ASSERT_EQ(zx::channel::create(0, &ph1a, &ph1b), ZX_OK);
  fidl::ClientEnd<provider::Provider> provider1_h{std::move(ph1b)};
  auto result1 =
      registry_ptr->RegisterProvider({{std::move(provider1_h), kProvider1Pid, kProvider1Name}});
  ASSERT_TRUE(result1.is_ok());

  zx::channel ph2a, ph2b;
  ASSERT_EQ(zx::channel::create(0, &ph2a, &ph2b), ZX_OK);
  fidl::ClientEnd<provider::ProviderV2> provider2_h{std::move(ph2b)};
  auto result2 =
      registry_ptr->RegisterV2({{std::move(provider2_h), kProvider2Pid, kProvider2Name}});
  ASSERT_TRUE(result2.is_ok());

  // Provider registrations come in on a different channel than
  // |GetProviders()|. Make sure the providers are registered before we try
  // to fetch a list of them.
  RunLoopUntilIdle();

  FX_LOGS(DEBUG) << "Providers registered";

  ConnectToProvisionerService();
  std::vector<fuchsia_tracing_controller::ProviderInfo> providers;
  provisioner_client()->GetProviders().ThenExactlyOnce([&providers](auto& result) {
    ASSERT_TRUE(result.is_ok());
    providers = std::move(result.value().providers());
  });
  RunLoopUntilIdle();

  EXPECT_EQ(providers.size(), 2u);
  for (const auto& p : providers) {
    EXPECT_TRUE(p.id().has_value());
    EXPECT_TRUE(p.pid().has_value());
    EXPECT_TRUE(p.name().has_value());
    if (p.pid().has_value()) {
      switch (p.pid().value()) {
        case kProvider1Pid:
          EXPECT_STREQ(p.name().value().c_str(), kProvider1Name);
          break;
        case kProvider2Pid:
          EXPECT_STREQ(p.name().value().c_str(), kProvider2Name);
          break;
        default:
          EXPECT_TRUE(false) << "Unexpected provider id";
          break;
      }
    }
  }
}

TEST_F(TraceManagerTest, AddFakeProviders) {
  ConnectToProvisionerService();

  FakeProvider* provider1;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider1));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);

  FakeProviderV2* provider2;
  ASSERT_TRUE(AddFakeProviderV2(kProvider2Pid, kProvider2Name, &provider2));
  EXPECT_EQ(fake_provider_bindings().size(), 2u);

  // Provider registrations come in on a different channel than
  // |GetProviders()|. Make sure the providers are registered before we try
  // to fetch a list of them.
  RunLoopUntilIdle();

  FX_LOGS(DEBUG) << "Providers registered";

  std::vector<fuchsia_tracing_controller::ProviderInfo> providers;
  provisioner_client()->GetProviders().ThenExactlyOnce([&providers](auto& result) {
    ASSERT_TRUE(result.is_ok());
    providers = std::move(result.value().providers());
  });
  RunLoopUntilIdle();

  EXPECT_EQ(providers.size(), 2u);
  for (const auto& p : providers) {
    EXPECT_TRUE(p.id().has_value());
    EXPECT_TRUE(p.pid().has_value());
    EXPECT_TRUE(p.name().has_value());
    if (p.pid().has_value()) {
      switch (p.pid().value()) {
        case kProvider1Pid:
          EXPECT_STREQ(p.name().value().c_str(), kProvider1Name);
          break;
        case kProvider2Pid:
          EXPECT_STREQ(p.name().value().c_str(), kProvider2Name);
          break;
        default:
          EXPECT_TRUE(false) << "Unexpected provider id";
          break;
      }
    }
  }
}

TEST_F(TraceManagerTest, GetKnownCategories) {
  ConnectToProvisionerService();

  FakeProvider* provider1;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider1));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);
  provider1->SetKnownCategories({
      {{.name = "foo"}},
      {{.name = "bar"}},
      {{.name = "provider1_category", .description = "description1"}},
  });

  FakeProviderV2* provider2;
  ASSERT_TRUE(AddFakeProviderV2(kProvider2Pid, kProvider2Name, &provider2));
  EXPECT_EQ(fake_provider_bindings().size(), 2u);
  provider2->SetKnownCategories({
      {{.name = "foo"}},
      {{.name = "bar"}},
      {{.name = "provider2_category", .description = "description2"}},
  });

  // Provider registrations come in on a different channel than
  // |GetProviders()|. Make sure the providers are registered before we try
  // to fetch a list of them.
  RunLoopUntilIdle();

  FX_LOGS(DEBUG) << "Providers registered";

  std::vector<fuchsia_tracing::KnownCategory> known_categories;
  provisioner_client()->GetKnownCategories().ThenExactlyOnce([&known_categories](auto& result) {
    ASSERT_TRUE(result.is_ok());
    known_categories = std::move(result.value().categories());
  });
  RunLoopUntilIdle();

  std::vector<fuchsia_tracing::KnownCategory> expected_categories = {
      {{.name = "provider2_category", .description = "description2"}},
      {{.name = "provider1_category", .description = "description1"}},
      {{.name = "bar"}},
      {{.name = "foo"}},
      {{.name = "test", .description = "Test category"}},
  };
  auto comparator = [](const fuchsia_tracing::KnownCategory& a,
                       const fuchsia_tracing::KnownCategory& b) {
    if (a.name() != b.name()) {
      return a.name() < b.name();
    }
    return a.description() < b.description();
  };
  std::sort(known_categories.begin(), known_categories.end(), comparator);
  std::sort(expected_categories.begin(), expected_categories.end(), comparator);

  ASSERT_EQ(expected_categories.size(), known_categories.size());
  for (size_t i = 0; i < expected_categories.size(); ++i) {
    EXPECT_EQ(expected_categories[i].name(), known_categories[i].name());
    EXPECT_EQ(expected_categories[i].description(), known_categories[i].description());
  }
}

TEST_F(TraceManagerTest, GetKnownCategoriesTimeout) {
  ConnectToProvisionerService();

  FakeProvider* provider1;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider1));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);
  provider1->SetKnownCategories({
      {{.name = "foo"}},
      {{.name = "bar"}},
      {{.name = "provider1_category", .description = "description1"}},
  });

  FakeProviderV2* provider2;
  ASSERT_TRUE(AddFakeProviderV2(kProvider2Pid, kProvider2Name, &provider2));
  EXPECT_EQ(fake_provider_bindings().size(), 2u);
  provider2->SetKnownCategories({
      {{.name = "foo"}},
      {{.name = "bar"}},
      {{.name = "provider2_category", .description = "description2"}},
  });

  FakeProviderV2* provider3;
  ASSERT_TRUE(AddFakeProviderV2(kProvider3Pid, kProvider3Name, &provider3));
  EXPECT_EQ(fake_provider_bindings().size(), 3u);
  provider3->SetKnownCategories({
      {{.name = "foo"}},
      {{.name = "bar"}},
      {{.name = "provider3_category", .description = "description3"}},
  });

  // Provider registrations come in on a different channel than
  // |GetProviders()|. Make sure the providers are registered before we try
  // to fetch a list of them.
  RunLoopUntilIdle();

  FX_LOGS(DEBUG) << "Providers registered";

  provider2->MarkUnresponsive();
  provider3->MarkUnresponsive();
  std::vector<fuchsia_tracing::KnownCategory> known_categories;
  provisioner_client()->GetKnownCategories().ThenExactlyOnce([&known_categories](auto& result) {
    ASSERT_TRUE(result.is_ok());
    known_categories = std::move(result.value().categories());
  });
  RunLoopFor(zx::sec(2));

  std::vector<fuchsia_tracing::KnownCategory> expected_categories = {
      {{.name = "provider1_category", .description = "description1"}},
      {{.name = "bar"}},
      {{.name = "foo"}},
      {{.name = "test", .description = "Test category"}},
  };
  auto comparator = [](const fuchsia_tracing::KnownCategory& a,
                       const fuchsia_tracing::KnownCategory& b) {
    if (a.name() != b.name()) {
      return a.name() < b.name();
    }
    return a.description() < b.description();
  };
  std::sort(known_categories.begin(), known_categories.end(), comparator);
  std::sort(expected_categories.begin(), expected_categories.end(), comparator);

  ASSERT_EQ(expected_categories.size(), known_categories.size());
  for (size_t i = 0; i < expected_categories.size(); ++i) {
    EXPECT_EQ(expected_categories[i].name(), known_categories[i].name());
    EXPECT_EQ(expected_categories[i].description(), known_categories[i].description());
  }
}

}  // namespace test
}  // namespace tracing
