// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_MOCK_DEBUGDATA_H_
#define LIB_LD_TESTING_MOCK_DEBUGDATA_H_

#include <fidl/fuchsia.debugdata/cpp/fidl.h>
#include <lib/zx/vmo.h>
#include <zircon/status.h>

#include <array>
#include <memory>
#include <string_view>

#include <gmock/gmock.h>

namespace async {
class Loop;
}  // namespace async

namespace ld::testing {

// This can be passed directly to fidl::BindServer after priming, either as an
// unowned pointer or via std::unique_ptr.  The one-method FIDL server is just
// a proxy for the one MOCK_METHOD.
class MockDebugdata : public fidl::Server<fuchsia_debugdata::Publisher> {
 public:
  // Use EXPECT_CALL for this method.
  MOCK_METHOD(void, Publish, (std::string_view data_sink, zx::vmo data, zx::eventpair token));

 private:
  void Publish(PublishRequest& request, PublishCompleter::Sync&) override;
};

// This is a matcher for use in EXPECT_CALL in the place of any zx::... handle
// type argument.  Its argument is a matcher applied against the VMO name as a
// std::string.
MATCHER_P(ObjNameMatches, matcher,
          "Kernel object has name that " +
              ::testing::DescribeMatcher<std::string>(matcher, negation)) {
  std::array<char, ZX_MAX_NAME_LEN> buffer;
  zx_status_t status = arg.get_property(ZX_PROP_NAME, buffer.data(), buffer.size());
  if (status != ZX_OK) {
    *result_listener << "ZX_PROP_NAME: " << zx_status_get_string(status);
    return false;
  }
  std::string name{buffer.data(), buffer.size()};
  name = std::move(name).substr(0, name.find('\0'));
  *result_listener << "VMO handle " << arg.get() << " with name \"" << name << "\"";
  return ExplainMatchResult(matcher, std::move(name), result_listener);
}

// This is a matcher for use in EXPECT_CALL in the place of any zx::... handle
// type argument.  Its argument is a matcher applied against the KOID.
MATCHER_P(ObjKoidMatches, matcher,
          "Kernel object KOID " + ::testing::DescribeMatcher<zx_koid_t>(matcher, negation)) {
  zx_info_handle_basic_t info;
  zx_status_t status = arg.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    *result_listener << "ZX_INFO_HANDLE_BASIC: " << zx_status_get_string(status);
    return false;
  }
  *result_listener << "Object handle " << arg.get() << " with KOID " << info.koid;
  return ExplainMatchResult(matcher, info.koid, result_listener);
}

// This is a matcher for use in EXPECT_CALL in the place of the VMO argument.
// Its argument is a matcher applied against the VMO contents as std::string.
MATCHER_P(VmoContentsMatch, matcher,
          "VMO has contents that " + ::testing::DescribeMatcher<std::string>(matcher, negation)) {
  uint64_t size;
  zx_status_t status = arg.get_prop_content_size(&size);
  if (status != ZX_OK) {
    *result_listener << "ZX_PROP_VMO_CONTENT_SIZE: " << zx_status_get_string(status);
    return false;
  }
  std::string contents;
  contents.resize(size);
  status = arg.read(contents.data(), 0, contents.size());
  if (status != ZX_OK) {
    *result_listener << "zx_vmo_read: " << zx_status_get_string(status);
    return false;
  }
  *result_listener << "VMO handle " << arg.get() << " with contents of " << size << " bytes";
  return ExplainMatchResult(matcher, std::move(contents), result_listener);
}

// This implements a fake directory meant to be used as /svc for a test.
// Arbitrary protocol endpoints can be added to it.
// Example use:
// ```
// auto mock = std::make_unique<ld::testing::MockDebugdata>();
// EXPECT_CALL(*mock, "data-sink", ObjNameMatches("vmo-name"), _);
// ld::testing::MockSvcDirectory svc_dir;
// svc_dir.Init();
// svc_dir.AddEntry<fuchsia_debugdata::Publisher>(std::move(mock));
// ... // Send pipelined Open containing Publish message, etc.
// svc_dir.loop().RunUntilIdle(); // Drain messages just sent.
// ... // On destruction the mock will check it got the expected Publish.
// ```
class MockSvcDirectory {
 public:
  MockSvcDirectory();
  MockSvcDirectory(MockSvcDirectory&&);
  ~MockSvcDirectory();

  // This must be called first and gets gtest assertion failures for errors.
  void Init();

  // This must be run to drain messages at some point.
  async::Loop& loop();

  // The explicit template parameter is some FIDL protocol bindings class.
  // The callable argument is passed to fidl::BindServer.
  template <class Protocol, typename ServerImplPtr>
  void AddEntry(ServerImplPtr&& server_impl) {
    auto metaconnector = [impl = std::forward<ServerImplPtr>(server_impl)](
                             async_dispatcher_t* dispatcher) mutable -> Connector {
      return [dispatcher, impl = std::forward<ServerImplPtr>(impl)](
                 zx::channel channel) mutable -> zx_status_t {
        fidl::BindServer(dispatcher, fidl::ServerEnd<Protocol>(std::move(channel)),
                         std::move(impl));
        return ZX_OK;
      };
    };
    AddEntry(fidl::DiscoverableProtocolName<Protocol>, std::move(metaconnector));
  }

  // This consumes the server end, serving the mock directory FIDL protocol.
  void Serve(fidl::ServerEnd<fuchsia_io::Directory> server_end);

  // This mints a fresh client end to the mock directory FIDL protocol.
  void Serve(fidl::ClientEnd<fuchsia_io::Directory>& client_end);

 private:
  class Impl;

  using Connector = fit::function<zx_status_t(zx::channel)>;
  using MetaConnector = fit::function<Connector(async_dispatcher_t*)>;

  void AddEntry(std::string_view, MetaConnector);

  std::unique_ptr<Impl> impl_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TESTING_MOCK_DEBUGDATA_H_
