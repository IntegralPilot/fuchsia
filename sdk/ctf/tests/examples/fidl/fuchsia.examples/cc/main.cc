// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START example]
#include <fuchsia/component/cpp/fidl.h>
#include <fuchsia/component/sandbox/cpp/fidl.h>
#include <fuchsia/examples/cpp/fidl.h>
#include <fuchsia/testing/harness/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <atomic>
#include <string>

#include <test/example/cpp/fidl.h>
#include <zxtest/zxtest.h>

class FuchsiaExamplesTest : public zxtest::Test {
 public:
  ~FuchsiaExamplesTest() override = default;
};

// TODO(https://fxbug.dev/330610053): Once a C++ realm_proxy library is available, use
// that instead of this custom boilerplate
class InstalledNamespace {
 public:
  InstalledNamespace(std::string prefix, zx::channel realm_factory)
      : prefix_(std::move(prefix)), realm_factory_(std::move(realm_factory)) {}
  ~InstalledNamespace() {
    fdio_ns_t* ns;
    EXPECT_EQ(fdio_ns_get_installed(&ns), ZX_OK);
    EXPECT_EQ(fdio_ns_unbind(ns, prefix_.c_str()), ZX_OK);
  }

  template <typename Interface>
  zx_status_t Connect(fidl::InterfaceRequest<Interface> request,
                      const std::string& interface_name = Interface::Name_) const {
    return Connect(interface_name, request.TakeChannel());
  }

  zx_status_t Connect(const std::string& interface_name, zx::channel request) const {
    const std::string path = prefix_ + "/" + interface_name;
    return fdio_service_connect(path.c_str(), request.release());
  }

  static InstalledNamespace Create(sys::ComponentContext* context);
  const std::string& prefix() const { return prefix_; }

 private:
  std::string prefix_;
  /// This is not used, but it keeps the RealmFactory connection alive.
  ///
  /// The RealmFactory server may use this connection to pin the lifetime of the realm created
  /// for the test.
  zx::channel realm_factory_;
};

namespace {
std::atomic_int32_t namespace_ctr{1};
}  // namespace

template <typename Interface>
InstalledNamespace ExtendNamespace(
    sys::ComponentContext* context, fidl::InterfaceHandle<Interface> realm_factory,
#if FUCHSIA_API_LEVEL_LESS_THAN(HEAD)
    fidl::InterfaceHandle<::fuchsia::component::sandbox::Dictionary> dictionary) {
#else
    zx::eventpair dictionary) {
#endif
  std::string prefix = std::string("/dict-") + std::to_string(namespace_ctr++);
  fuchsia::component::NamespaceSyncPtr namespace_proxy;
  EXPECT_OK(context->svc()->Connect(namespace_proxy.NewRequest()));
#if FUCHSIA_API_LEVEL_LESS_THAN(HEAD)
  std::vector<fuchsia::component::NamespaceInputEntry> entries;
  entries.emplace_back(fuchsia::component::NamespaceInputEntry{
      .path = prefix,
      .dictionary = std::move(dictionary),
  });
  fuchsia::component::Namespace_Create_Result result;
  EXPECT_OK(namespace_proxy->Create(std::move(entries), &result));
#else
  std::vector<fuchsia::component::NamespaceInputEntry2> entries;
  entries.emplace_back(fuchsia::component::NamespaceInputEntry2{
      .path = prefix,
      .capability = std::move(dictionary),
  });
  fuchsia::component::Namespace_Create2_Result result;
  EXPECT_OK(namespace_proxy->Create2(std::move(entries), &result));
#endif
  EXPECT_TRUE(!result.is_err());
  std::vector<fuchsia::component::NamespaceEntry> namespace_entries =
      std::move(result.response().entries);
  EXPECT_EQ(namespace_entries.size(), 1);
  auto& entry = namespace_entries[0];
  EXPECT_TRUE(entry.has_path() && entry.has_directory());
  EXPECT_EQ(entry.path(), prefix);
  fdio_ns_t* ns;
  EXPECT_OK(fdio_ns_get_installed(&ns));
  zx_handle_t dir_handle = entry.mutable_directory()->TakeChannel().release();
  EXPECT_OK(fdio_ns_bind(ns, prefix.c_str(), dir_handle));
  return InstalledNamespace(std::move(prefix), realm_factory.TakeChannel());
}

TEST(FuchsiaExamplesTest, Echo) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  test::example::RealmFactorySyncPtr realm_factory;
  auto context = sys::ComponentContext::Create();
  EXPECT_OK(context->svc()->Connect(realm_factory.NewRequest()));

#if FUCHSIA_API_LEVEL_LESS_THAN(HEAD)
  fuchsia::component::sandbox::DictionarySyncPtr dictionary;
  test::example::RealmFactory_CreateRealm_Result result;
  test::example::RealmOptions options;
  EXPECT_OK(realm_factory->CreateRealm(std::move(options), dictionary.NewRequest(), &result));
  auto test_ns = ExtendNamespace(context.get(), realm_factory.Unbind(), dictionary.Unbind());
#else
  zx::eventpair dictionary;
  zx::eventpair dictionary_other_end;
  zx_status_t status = zx::eventpair::create(0, &dictionary, &dictionary_other_end);
  ZX_ASSERT(status == ZX_OK);
  test::example::RealmFactory_CreateRealm2_Result result;
  test::example::RealmOptions options;
  EXPECT_OK(realm_factory->CreateRealm2(std::move(options), std::move(dictionary), &result));
  auto test_ns =
      ExtendNamespace(context.get(), realm_factory.Unbind(), std::move(dictionary_other_end));
#endif

  fuchsia::examples::EchoSyncPtr echo;
  EXPECT_OK(test_ns.Connect(echo.NewRequest()));

  std::string response;
  EXPECT_OK(echo->EchoString("hello", &response));
  EXPECT_STREQ(response.c_str(), "hello");
}
// [END example]
