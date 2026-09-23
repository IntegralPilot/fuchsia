// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/component/cpp/fidl.h>
#include <fuchsia/component/sandbox/cpp/fidl.h>
#include <fuchsia/settings/cpp/fidl.h>
#include <fuchsia/settings/test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/service_directory.h>

#include <atomic>
#include <optional>
#include <string>

#include <zxtest/zxtest.h>

namespace {

// TODO(https://fxbug.dev/330610053): Once a C++ realm_proxy library is available, use
// that instead of this custom boilerplate
class InstalledNamespace {
 public:
  InstalledNamespace(std::string prefix, zx::channel realm_factory)
      : prefix_(std::move(prefix)), realm_factory_(std::move(realm_factory)) {
    ZX_ASSERT_MSG(!prefix_.empty(), "prefix cannot be empty");
  }
  InstalledNamespace(InstalledNamespace&& rhs) = default;
  ~InstalledNamespace() {
    if (prefix_.empty()) {
      return;
    }
    fdio_ns_t* ns;
    EXPECT_EQ(fdio_ns_get_installed(&ns), ZX_OK);
    EXPECT_EQ(fdio_ns_unbind(ns, prefix_.c_str()), ZX_OK);
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

std::atomic_int32_t namespace_ctr{1};

class PrivacyTest : public zxtest::Test {
 protected:
  void SetUp() override {
    EXPECT_EQ(ZX_OK, service->Connect(realm_factory.NewRequest()));

    fuchsia::settings::test::RealmOptions options;
    fuchsia::settings::test::RealmFactory_CreateRealm2_Result result1;
    fuchsia::component::sandbox::DictionarySyncPtr dictionary;
    EXPECT_OK(realm_factory->CreateRealm2(std::move(options), dictionary.NewRequest(), &result1));
    test_ns_.emplace(ExtendNamespace(realm_factory.Unbind(), dictionary.Unbind()));
    EXPECT_OK(test_ns_->Connect("fuchsia.settings.Privacy", privacy.NewRequest().TakeChannel()));
  }

  void TearDown() override {
    fuchsia::settings::PrivacySettings settings;
    settings.set_user_data_sharing_consent(GetInitValue());
    fuchsia::settings::Privacy_Set_Result result;
    fuchsia::settings::PrivacySettings got_settings;
    // Set back to the initial settings and verify.
    EXPECT_EQ(ZX_OK, privacy->Set(std::move(settings), &result));
    EXPECT_EQ(ZX_OK, privacy->Watch(&got_settings));
    EXPECT_TRUE(got_settings.user_data_sharing_consent());
  }

  bool GetInitValue() const { return this->init_user_data_sharing_consent; }

  fuchsia::settings::PrivacySyncPtr privacy;

 private:
  // TODO(https://fxbug.dev/330610053): Once a C++ realm_proxy library is available, use
  // that instead of this custom boilerplate
  template <typename Interface>
  InstalledNamespace ExtendNamespace(
      fidl::InterfaceHandle<Interface> realm_factory,
      fidl::InterfaceHandle<::fuchsia::component::sandbox::Dictionary> dictionary) {
    std::string prefix = std::string("/dict-") + std::to_string(namespace_ctr++);
    fuchsia::component::NamespaceSyncPtr namespace_proxy;
    EXPECT_OK(service->Connect(namespace_proxy.NewRequest()));
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

  fuchsia::settings::test::RealmFactorySyncPtr realm_factory;
  std::shared_ptr<sys::ServiceDirectory> service = sys::ServiceDirectory::CreateFromNamespace();
  std::optional<InstalledNamespace> test_ns_;
  bool init_user_data_sharing_consent = true;
};

// Tests that Set() results in an update to privacy settings.
TEST_F(PrivacyTest, Set) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  // Setup initial PrivacySettings.
  fuchsia::settings::PrivacySettings settings;
  fuchsia::settings::Privacy_Set_Result result;
  settings.set_user_data_sharing_consent(GetInitValue());
  EXPECT_EQ(ZX_OK, privacy->Set(std::move(settings), &result));

  // Verify initial settings.
  fuchsia::settings::PrivacySettings got_settings;
  EXPECT_EQ(ZX_OK, privacy->Watch(&got_settings));
  EXPECT_TRUE(got_settings.user_data_sharing_consent());

  // Flip the settings.
  fuchsia::settings::PrivacySettings new_settings;
  new_settings.set_user_data_sharing_consent(false);
  EXPECT_EQ(ZX_OK, privacy->Set(std::move(new_settings), &result));

  // Verify the new settings.
  EXPECT_EQ(ZX_OK, privacy->Watch(&got_settings));
  EXPECT_FALSE(got_settings.user_data_sharing_consent());
}

}  // namespace
