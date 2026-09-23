// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TESTING_CPP_TEST_NODE_H_
#define LIB_DRIVER_TESTING_CPP_TEST_NODE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <zircon/availability.h>

namespace fdf_testing {

// This class serves as the FIDL servers for the fuchsia_driver_framework::Node and
// fuchsia_driver_framework::NodeController to a driver under test.
//
// In a regular driver environment, this is provided by the driver framework's driver manager.
// The Node FIDL is how drivers communicate with the driver framework to add child nodes into the
// driver topology.
//
// The TestNode is part of the unit test's environment that is given to the driver under test using
// it's start args. Therefore this class is where the test acquires the start args to give to the
// driver, using CreateStartArgsAndServe. The result of this contains three values:
//  - The actual start args for the driver to be given to the driver's Start.
//  - A server end of the incoming directory the driver will use. This must be passed into the
//    |fdf_testing::TestEnvironment::Initialize| function.
//  - A client end to the outgoing directory of the driver. This can be used by the test to
//    talk to FIDLs provided by the driver under test.
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from a synchronized dispatcher.
// See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
//
// If the dispatcher used for it is the foreground dispatcher, the TestNode does not need to be
// wrapped in a DispatcherBound.
//
// If the dispatcher is a background dispatcher, the suggestion is to wrap this inside of an
// |async_patterns::TestDispatcherBound|.
class TestNode final : public fidl::WireServer<fuchsia_driver_framework::NodeController>,
                       public fidl::WireServer<fuchsia_driver_framework::Node> {
 public:
  // This can be used to access child nodes that a driver has created.
  using ChildrenMap = std::unordered_map<std::string, TestNode>;

  // Used to see the result of using NodeController::RequestBind.
  struct BindData {
    bool force_rebind;
    std::string driver_url_suffix;
  };

  // This is the bundle returned from |CreateStartArgsAndServe|.
  // As described above, start_args goes to the driver, incoming_directory_server goes to the
  // environment, and outgoing_directory_client may be used by the test.
  struct CreateStartArgsResult {
    fuchsia_driver_framework::DriverStartArgs start_args;
    fidl::ServerEnd<fuchsia_io::Directory> incoming_directory_server;
    fidl::ClientEnd<fuchsia_io::Directory> outgoing_directory_client;
  };

  // If no dispatcher is provided, this will try to use the current fdf_dispatcher. If there
  // is also no fdf_dispatcher, it will try to use the thread's default async_dispatcher.
  explicit TestNode(std::string name, async_dispatcher_t* dispatcher = nullptr);

  ~TestNode() override;

  // Gets the children created by the driver on this node.
  ChildrenMap& children() { return children_; }

  // Gets the name of the node.
  const std::string& name() const { return name_; }

  // Create a channel pair, serve the server end, and return the client end.
  // This method is thread-unsafe. Must be called from the same context as the dispatcher.
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::Node>> CreateNodeChannel();

  // Serve the given server end.
  // This method is thread-unsafe. Must be called from the same context as the dispatcher.
  zx::result<> Serve(fidl::ServerEnd<fuchsia_driver_framework::Node> server_end);

  // Creates the start args for a driver, and serve the fdf::Node in it.
  zx::result<CreateStartArgsResult> CreateStartArgsAndServe();

  // Connects to the devfs device this node is serving.
  zx::result<zx::channel> ConnectToDevice();

  // Whether this node has started serving the fdf::Node, this happens when |Serve|
  // or |CreateNodeChannel| has been called.
  bool HasNode() {
    std::lock_guard guard(checker_);
    return node_binding_.has_value();
  }

#if FUCHSIA_API_LEVEL_LESS_THAN(27)
  // Get the node properties that this node was created with. Can be used to validate that a driver
  // is creating valid child nodes.
  std::vector<fuchsia_driver_framework::NodeProperty> GetProperties() const {
    std::lock_guard guard(checker_);
    return properties_;
  }
#else
  // Get the node properties that this node was created with. Can be used to validate that a driver
  // is creating valid child nodes.
  std::vector<fuchsia_driver_framework::NodeProperty2> GetProperties() const {
    std::lock_guard guard(checker_);
    return properties_;
  }
#endif

  // Gets the bind data that were stored as part of NodeController::RequestBind calls.
  std::vector<BindData> GetBindData() const {
    std::lock_guard guard(checker_);
    return bind_data_;
  }

  // Get the dispatcher this Node object lives and serves FIDLs on.
  async_dispatcher_t* dispatcher() const { return dispatcher_; }

 private:
  void AddChild(AddChildRequestView request, AddChildCompleter::Sync& completer) override;

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  void ProvideResource(
      fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceRequestView request,
      fidl::WireServer<fuchsia_driver_framework::Node>::ProvideResourceCompleter::Sync& completer)
      override;
#endif

  void Remove(RemoveCompleter::Sync& completer) override { RemoveFromParent(); }
  void RequestBind(RequestBindRequestView request, RequestBindCompleter::Sync& completer) override;
#if FUCHSIA_API_LEVEL_AT_LEAST(29)
  void WaitForDriver(WaitForDriverCompleter::Sync& completer) override;
#endif

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_framework::NodeController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_framework::Node> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  void SetParent(TestNode* parent,
                 fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller);

#if FUCHSIA_API_LEVEL_LESS_THAN(27)
  void SetProperties(std::vector<fuchsia_driver_framework::NodeProperty> properties);
#else
  void SetProperties(std::vector<fuchsia_driver_framework::NodeProperty2> properties);
#endif

  void RemoveFromParent();

  void RemoveChild(const std::string& name);

  void set_devfs_connector_client(fidl::ClientEnd<fuchsia_device_fs::Connector> client) {
    std::lock_guard guard(checker_);
    devfs_connector_client_.Bind(std::move(client), dispatcher_);
  }

  std::optional<fidl::ServerBinding<fuchsia_driver_framework::Node>> node_binding_
      __TA_GUARDED(checker_);
  std::optional<fidl::ServerBinding<fuchsia_driver_framework::NodeController>> controller_binding_
      __TA_GUARDED(checker_);

  async_dispatcher_t* dispatcher_;

#if FUCHSIA_API_LEVEL_LESS_THAN(27)
  std::vector<fuchsia_driver_framework::NodeProperty> properties_ __TA_GUARDED(checker_);
#else
  std::vector<fuchsia_driver_framework::NodeProperty2> properties_ __TA_GUARDED(checker_);
#endif

  std::vector<BindData> bind_data_ __TA_GUARDED(checker_);
  std::string name_ __TA_GUARDED(checker_);
  std::optional<std::reference_wrapper<TestNode>> parent_ __TA_GUARDED(checker_);
  ChildrenMap children_ __TA_GUARDED(checker_);
  fidl::WireClient<fuchsia_device_fs::Connector> devfs_connector_client_ __TA_GUARDED(checker_);
  async::synchronization_checker checker_;
};

}  // namespace fdf_testing

#endif  // LIB_DRIVER_TESTING_CPP_TEST_NODE_H_
