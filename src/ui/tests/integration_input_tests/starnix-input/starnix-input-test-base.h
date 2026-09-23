// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zx/socket.h>
#include <lib/zx/time.h>
#include <zircon/processargs.h>

#include "relay-api.h"
#include "src/ui/testing/util/portable_ui_test.h"

namespace starnix_input_test {

// Types imported for the realm_builder library.
using component_testing::ChildRef;
using component_testing::Directory;
using component_testing::ParentRef;
using component_testing::Route;

// Alias for Component child name as provided to Realm Builder.
using ChildName = std::string;

// Timeout for reading from input dump socket.
constexpr zx::duration kSocketTimeout = zx::min(3);

struct StdIOSocket {
  zx::socket in_socket;
  zx::socket out_socket;
};

class StarnixInputTestBase : public ui_testing::PortableUITest {
 protected:
  struct EvDevPacket {
    // The event timestamp received by Starnix, from Fuchsia.
    int64_t sec;
    int64_t usec;
    // * For an overview of the following fields, see
    //   https://kernel.org/doc/html/latest/input/input.html#event-interface
    // * For details on the constants relevant to Starnix input events, see
    //   https://kernel.org/doc/html/latest/input/event-codes.html
    uint16_t type;
    uint16_t code;
    int32_t value;
  };

  // To satisfy ::testing::Test
  void TearDown() override {
    realm_event_handler_.Stop();
    ui_testing::PortableUITest::TearDown();
  }

  // Launches `input_dump.cc`, connecting its `stdout` to `out_socket`.
  // Then waits for `input_dump.cc` to report that it is ready to receive
  // input events.
  StdIOSocket LaunchDumper() {
    // Create a socket for communicating with `input_dump`, and store it in
    // a collection of `HandleInfo`s.
    std::vector<fuchsia_process::HandleInfo> numbered_handles;
    zx::socket out_remote_socket;
    zx::socket out_socket;
    zx::socket in_remote_socket;
    zx::socket in_socket;
    zx_status_t sock_res;

    // stdout
    sock_res = zx::socket::create(ZX_SOCKET_DATAGRAM, &out_socket, &out_remote_socket);
    FX_CHECK(sock_res == ZX_OK) << "Creating socket failed: " << zx_status_get_string(sock_res);
    numbered_handles.push_back(fuchsia_process::HandleInfo{
        {.handle = zx::handle(std::move(out_remote_socket)), .id = PA_HND(PA_FD, STDOUT_FILENO)}});

    // stdin
    sock_res = zx::socket::create(ZX_SOCKET_DATAGRAM, &in_socket, &in_remote_socket);
    FX_CHECK(sock_res == ZX_OK) << "Creating socket failed: " << zx_status_get_string(sock_res);
    numbered_handles.push_back(fuchsia_process::HandleInfo{
        {.handle = zx::handle(std::move(in_remote_socket)), .id = PA_HND(PA_FD, STDIN_FILENO)}});

    // Launch the child.
    FX_LOGS(INFO) << "Launching input_dump";
    std::optional<fidl::Result<fuchsia_component::Realm::CreateChild>> create_child_status;
    zx::result<fidl::ClientEnd<fuchsia_component::Realm>> realm_proxy =
        realm_root()->component().Connect<fuchsia_component::Realm>();
    if (realm_proxy.is_error()) {
      FX_LOGS(FATAL) << "Failed to connect to Realm server: "
                     << zx_status_get_string(realm_proxy.error_value());
    }
    realm_client_ =
        fidl::Client(std::move(realm_proxy.value()), dispatcher(), &realm_event_handler_);
    realm_client_
        ->CreateChild({fuchsia_component_decl::CollectionRef(
                           {{.name = "debian_userspace"}}),  // Declared in `debian_container.cml`
                       fuchsia_component_decl::Child(
                           {{.name = "input_dump",
                             .url = "#meta/input_dump.cm",
                             .startup = fuchsia_component_decl::StartupMode::kLazy}}),
                       // The `ChildArgs` enable tests to read from the stdout of `input_dump.cc`.
                       fuchsia_component::CreateChildArgs(
                           {{.numbered_handles = std::move(numbered_handles)}})})
        .ThenExactlyOnce([&](auto result) { create_child_status = std::move(result); });
    RunLoopUntil([&] { return create_child_status.has_value(); });

    // Check that launching succeeded.
    const auto& status = create_child_status.value();
    FX_CHECK(!status.is_error()) << "CreateChild() returned error " << status.error_value();

    return {.in_socket = std::move(in_socket), .out_socket = std::move(out_socket)};
  }

  void WaitForMessageFromInputDump(zx::socket& out_socket, const std::string& message) {
    FX_LOGS(INFO) << "Waiting message " << message << " from input_dump";
    auto packet = BlockingReadFromInputDump(out_socket);
    ASSERT_EQ(packet, message) << "Got \"" << packet.data() << "\" with size " << packet.size();
  }

  void WriteMessageToSocket(zx::socket& in_socket, const std::string& message) {
    size_t wrote;
    in_socket.write(0, message.data(), message.size(), &wrote);
    ASSERT_EQ(wrote, message.size());
  }

  std::vector<EvDevPacket> GetEvDevPackets(zx::socket& out_socket) {
    std::vector<EvDevPacket> ev_pkts;
    std::string packets = BlockingReadFromInputDump(out_socket);
    std::size_t next = packets.find(relay_api::kEventDelimiter);
    while (next != std::string::npos) {
      packets = packets.substr(next);
      EvDevPacket ev_pkt{};
      int res = sscanf(packets.data(), relay_api::kEventFormat, &ev_pkt.sec, &ev_pkt.usec,
                       &ev_pkt.type, &ev_pkt.code, &ev_pkt.value);
      FX_CHECK(res == 5) << "Got " << res << " fields, but wanted 5";
      ev_pkts.push_back(ev_pkt);
      next = packets.find(relay_api::kEventDelimiter, relay_api::kEventDelimiter.size());
    }

    return ev_pkts;
  }

 private:
  static constexpr auto kDebianRealm = "debian-realm";
  static constexpr auto kDebianRealmUrl = "#meta/debian_realm.cm";

  class RealmEventHandler : public fidl::AsyncEventHandler<fuchsia_component::Realm> {
   public:
    // Ignores any later errors on `this`. Used to avoid false-failures during
    // test teardown.
    void Stop() { running_ = false; }

    void on_fidl_error(fidl::UnbindInfo error) override {
      if (running_) {
        FX_LOGS(FATAL) << "Error on Realm client: " << error;
      }
    }

   private:
    bool running_ = true;
  };

  // To satisfy ui_testing::PortableUITest
  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

  std::vector<std::pair<ChildName, std::string>> GetTestComponents() override {
    return {
        std::make_pair(kDebianRealm, kDebianRealmUrl),
    };
  }

  std::vector<Route> GetTestRoutes() override {
    return {
        // Route global capabilities from parent to the Debian realm.
        {.capabilities = {Proto<fuchsia_kernel::VmexResource>(), Proto<fuchsia_sysmem::Allocator>(),
                          Proto<fuchsia_sysmem2::Allocator>(),
                          Proto<fuchsia_tracing_provider::Registry>()},
         .source = ParentRef(),
         .targets = {ChildRef{kDebianRealm}}},

        {.capabilities =
             {
                 Directory{
                     .name = "boot-kernel",
                     .type = fuchsia::component::decl::DependencyType::STRONG,
                 },
             },
         .source = ParentRef(),
         .targets = {ChildRef{kDebianRealm}}},

        // Route capabilities from test-ui-stack to the Debian realm.
        {.capabilities = {Proto<fuchsia_ui_composition::Allocator>(),
                          Proto<fuchsia_ui_composition::Flatland>(),
                          Proto<fuchsia_ui_display_singleton::Info>(),
                          Proto<fuchsia_element::GraphicalPresenter>()},
         .source = ui_testing::PortableUITest::kTestUIStackRef,
         .targets = {ChildRef{kDebianRealm}}},

        // Route capabilities from the Debian realm to the parent.
        {.capabilities =
             {// Allow this test to launch `input_dump` inside the Debian realm.
              Proto<fuchsia_component::Realm>()},
         .source = ChildRef{kDebianRealm},
         .targets = {ParentRef()}},
    };
  }

  // Reads a single piece of data from `input_dump.cc`, via `out_socket`.
  //
  // There's no framing protocol between these two programs, so calling
  // code must run in lock-step with `input_dump.cc`.
  //
  // In particular: the calling code must not send a second input event
  // until the calling code has read the response that `input_dump.cc`
  // sent for the first event.
  std::string BlockingReadFromInputDump(zx::socket& out_socket) {
    std::string buf(relay_api::kMaxPacketLen * relay_api::kDownUpNumPackets, '\0');
    size_t n_read{};
    zx_status_t res{};
    zx_signals_t actual_signals;

    FX_LOGS(INFO) << "Waiting for socket to be readable";
    res = out_socket.wait_one(ZX_SOCKET_READABLE, zx::deadline_after(kSocketTimeout),
                              &actual_signals);
    FX_CHECK(res == ZX_OK) << "wait_one() returned " << zx_status_get_string(res);
    FX_CHECK(actual_signals & ZX_SOCKET_READABLE)
        << "expected signals to include ZX_SOCKET_READABLE, but actual_signals=" << actual_signals;

    res = out_socket.read(/* options = */ 0, buf.data(), buf.capacity(), &n_read);
    FX_CHECK(res == ZX_OK) << "read() returned " << zx_status_get_string(res);
    buf.resize(n_read);

    FX_CHECK(buf != relay_api::kFailedMessage);
    return buf;
  }

  template <typename T>
  component_testing::Protocol Proto() {
    return {fidl::DiscoverableProtocolName<T>};
  }

  // Resources for communicating with the realm server.
  // * `realm_event_handler_` must live at least as long as `realm_client_`
  // * `realm_client_` is stored in the fixture to keep `input_dump` alive for the
  //   duration of the test
  RealmEventHandler realm_event_handler_;
  fidl::Client<fuchsia_component::Realm> realm_client_;
};

}  // namespace starnix_input_test
