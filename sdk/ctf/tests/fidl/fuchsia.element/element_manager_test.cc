// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/component/cpp/fidl.h>
#include <fuchsia/component/sandbox/cpp/fidl.h>
#include <fuchsia/element/cpp/fidl.h>
#include <fuchsia/element/test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/sys/cpp/component_context.h>

#include <atomic>
#include <string>

#include <zxtest/zxtest.h>

namespace {

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
  InstalledNamespace(InstalledNamespace&&) = default;
  InstalledNamespace& operator=(InstalledNamespace&&) = default;

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

std::atomic_int32_t namespace_ctr{1};

template <typename Interface>
InstalledNamespace ExtendNamespace(
    sys::ComponentContext* context, fidl::InterfaceHandle<Interface> realm_factory,
    fidl::InterfaceHandle<::fuchsia::component::sandbox::Dictionary> dictionary) {
  std::string prefix = std::string("/dict-") + std::to_string(namespace_ctr++);
  fuchsia::component::NamespaceSyncPtr namespace_proxy;
  EXPECT_OK(context->svc()->Connect(namespace_proxy.NewRequest()));
  std::vector<fuchsia::component::NamespaceInputEntry> entries;
  entries.emplace_back(fuchsia::component::NamespaceInputEntry{
      .path = prefix,
      .dictionary = std::move(dictionary),
  });
  fuchsia::component::Namespace_Create_Result result;
  EXPECT_OK(namespace_proxy->Create(std::move(entries), &result));
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

constexpr auto kReferenceElementV2Url = "#meta/reference-element.cm";

class ElementManagerTest : public zxtest::Test {};

// Tests that proposing an element returns a successful result.
TEST_F(ElementManagerTest, ProposeElement) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto context = sys::ComponentContext::Create();
  fuchsia::element::test::RealmFactorySyncPtr realm_factory;
  EXPECT_EQ(ZX_OK, context->svc()->Connect(realm_factory.NewRequest()));

  fuchsia::component::sandbox::DictionarySyncPtr dictionary;
  fuchsia::element::test::RealmFactory_CreateRealm2_Result result;
  fuchsia::element::test::RealmOptions options;
  EXPECT_OK(realm_factory->CreateRealm2(std::move(options), dictionary.NewRequest(), &result));
  auto test_ns = ExtendNamespace(context.get(), realm_factory.Unbind(), dictionary.Unbind());

  // TODO(kjharland): Create a helper macro to shorten this to one line.
  zx::channel local;
  zx::channel remote;
  ASSERT_OK(zx::channel::create(0, &local, &remote));
  EXPECT_OK(test_ns.Connect("fuchsia.element.Manager", std::move(remote)));

  fuchsia::element::ManagerPtr manager;
  manager.Bind(std::move(local));

  manager.set_error_handler([&](zx_status_t status) {
    EXPECT_EQ(ZX_OK, status);
    loop.Quit();
  });

  fuchsia::element::Spec spec;
  spec.set_component_url(kReferenceElementV2Url);

  bool is_proposed{false};
  manager->ProposeElement(std::move(spec), /*controller=*/nullptr,
                          [&](fuchsia::element::Manager_ProposeElement_Result result) {
                            EXPECT_FALSE(result.is_err());
                            is_proposed = true;
                            loop.Quit();
                          });

  loop.Run();

  EXPECT_TRUE(is_proposed);
}

}  // namespace
