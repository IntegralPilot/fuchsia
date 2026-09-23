// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_DRIVER_BASE2_H_
#define LIB_DRIVER_COMPONENT_CPP_DRIVER_BASE2_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <lib/component/outgoing/cpp/structured_config.h>
#include <lib/driver/component/cpp/resume_completer.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <lib/driver/component/cpp/stop_completer.h>
#include <lib/driver/component/cpp/suspend_completer.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/incoming/cpp/service_validator.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/node/cpp/add_child.h>
#include <lib/driver/node/cpp/provide-resource.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/stdcompat/span.h>
#include <zircon/availability.h>

#include <memory>
#include <string_view>

namespace fdf_internal {
template <typename DriverBaseImpl>
class DriverServer2;
}  // namespace fdf_internal

namespace fdf {

using DriverStartArgs = fuchsia_driver_framework::DriverStartArgs;

class DriverBase2;

class DriverContext {
 public:
  explicit DriverContext(fuchsia_driver_framework::DriverStartArgs start_args);

  const std::vector<fuchsia_driver_framework::Offer>& node_offers() {
    auto& node_offers = start_args_.node_offers();
    ZX_ASSERT(node_offers.has_value());
    return node_offers.value();
  }

  template <typename StructuredConfig>
    requires component::IsStructuredConfigV<StructuredConfig>
  StructuredConfig take_config() {
    std::optional config_vmo = std::move(start_args_.config());
    ZX_ASSERT_MSG(config_vmo.has_value(),
                  "Config VMO handle must be provided and cannot already have been taken.");
    return StructuredConfig::CreateFromVmo(std::move(config_vmo.value()));
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(29)
  zx::unowned_vmar vmar() {
    auto& vmar = start_args_.vmar();
    if (vmar.has_value()) {
      return vmar->borrow();
    }
    return zx::vmar::root_self();
  }
#endif

  // Used to access the incoming namespace of the driver. This allows connecting to both zircon and
  // driver transport incoming services.
  const Namespace& incoming() const {
    ZX_ASSERT(incoming_.get() != nullptr);
    return *incoming_;
  }

  Namespace& incoming() {
    ZX_ASSERT(incoming_.get() != nullptr);
    return *incoming_;
  }

  // Used to access the incoming namespace of the driver. This allows connecting to both zircon and
  // driver transport incoming services.
  std::unique_ptr<Namespace> take_incoming() {
    ZX_ASSERT(incoming_.get() != nullptr);
    return std::move(incoming_);
  }

  // The `/svc` directory in the incoming namespace.
  fidl::UnownedClientEnd<fuchsia_io::Directory> svc() const {
    ZX_ASSERT(incoming_.get() != nullptr);
    return incoming_->svc_dir();
  }

  // The program dictionary in the start args.
  // This is the `program` entry in the cml of the driver.
  const std::optional<fuchsia_data::Dictionary>& program() const { return start_args_.program(); }

  // The url field in the start args.
  // This is the URL of the package containing the driver. This is purely informational,
  // used only to provide data for inspect.
  const std::optional<std::string>& url() const { return start_args_.url(); }

  // The node_name field in the start args.
  // This is the name of the node that the driver is bound to.
  const std::optional<std::string>& node_name() const { return start_args_.node_name(); }

  // Returns the node properties of the node the driver is bound to or its parents.
  // Returns the node's own node properties if `parent_node_name` is "default" and the node is a
  // non-composite.
  // Returns the node's primary parent's node properties if `parent_node_name` is "default" and the
  // node is a composite.
  // Returns an empty vector if the parent does not exist.
  cpp20::span<const fuchsia_driver_framework::NodeProperty2> node_properties(
      const std::string& parent_node_name = "default") const;

  // Takes the node token for this driver. This should only be called once.
  zx::event take_node_token();

  // The symbols field in the start args.
  // These come from the driver that added |node|, and are filtered to the symbols requested in the
  // bind program.
  const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols() const {
    return start_args_.symbols();
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  // Takes the `fuchsia.power.broker/ElementRunner` channel for this driver's power element. The
  // caller is expected to run the power element.
  //
  // Returns std::nullopt if the driver did not specify `suspend_enabled` in its manifest's program
  // section, this is not a suspend enabled platform, or the channel was not provided to the driver
  // host.
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> take_power_element_runner();

  // Takes the `fuchsia.power.broker/Lessor` channel. This allows the caller to lease the power
  // element.
  //
  // Returns std::nullopt if the driver did not specify `suspend_enabled` in its manifest's program
  // section, this is not a suspend enabled platform, or the channel was not provided to the driver
  // host.
  std::optional<fidl::ClientEnd<fuchsia_power_broker::Lessor>> take_power_element_lessor();

  // Returns a copy of the power element token for the driver's power element. This can be called
  // repeatedly.
  //
  // Returns std::nullopt if the driver did not specify `suspend_enabled` in its manifest's program
  // section, this is not a suspend enabled platform, or the token was not provided by the driver
  // host.
  std::optional<fuchsia_power_broker::DependencyToken> power_element_token();

  // Whether the power handles were provided in the start args. If the handles are absent it means
  // either the driver's manifest did not request them or the driver did, but this is not a power-
  // enabled product.
  // TODO(https://fxbug.dev/495557052): Remove this API once this bug is closed.
  bool has_power_args();
#endif

  // Creates a component-wide Inspector for the driver.
  inspect::ComponentInspector CreateInspector(DriverBase2* driver,
                                              inspect::Inspector inspector = {}) const;

 private:
  friend DriverBase2;

  // This will enable validating service instance connection requests that are made to the incoming
  // namespace |incoming()|. It will ensure that the given service + instance combination is valid
  // before attempting to make a connection. If it is not a valid combination, |Connect()| attempts
  // on the namespace, will return a ZX_ERR_NOT_FOUND error immediately.
  //
  // This can be enabled by setting `service_connect_validation: "true"` in the driver cml's
  // `program` section.
  void EnableServiceValidator();

  fuchsia_driver_framework::DriverStartArgs start_args_;
  std::unique_ptr<Namespace> incoming_;
};

// |DriverBase| is an interface that drivers should inherit from. It provides methods
// for accessing the start args, as well as helper methods for common initialization tasks.
//
// There are four virtual methods:
// |Start| which must be overridden.
// |Stop| and the destructor |~DriverBase|, are optional to override.
//
// In order to work with the default FUCHSIA_DRIVER_EXPORT macro,
// classes which inherit from |DriverBase| must implement a constructor with the following
// signature and forward said parameters to the |DriverBase| base class:
//
//   T(DriverContext& context, fdf::UnownedSynchronizedDispatcher driver_dispatcher);
//
// The following illustrates an example:
//
// ```
// class MyDriver : public fdf::DriverBase2 {
//  public:
//   MyDriver(fdf::DriverContext& context, fdf::UnownedSynchronizedDispatcher driver_dispatcher)
//       : fdf::DriverBase("my_driver", context, std::move(driver_dispatcher)) {}
//
//   zx::result<> Start(DriverContext context) override {
//     context.incoming()->Connect(...);
//     outgoing()->AddService(...);
//     FDF_LOG(INFO, "hello world!");
//     inspector().Health().Ok();
//     node_client_.Bind(std::move(node()), dispatcher());
//
//     /* Ensure all capabilities offered have been added to the outgoing directory first. */
//     auto add_result = AddChild(...); if (add_result.is_error()) {
//       /* Releasing the node channel signals unbind to DF. */
//       node_client_.AsyncTeardown(); // Or node().reset() if we hadn't moved it into the client.
//       return add_result.take_error();
//     }
//
//     return zx::ok();
//   }
//  private:
//   fidl::Client<fuchsia_driver_framework::Node> node_client_;
// };
// ```
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from tasks
// running on the |driver_dispatcher|, and the dispatcher must be synchronized.
// See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
class DriverBase2 {
 public:
  // Gets the DriverBase instance from the given token. This is only intended for testing.
  template <typename DriverBaseImpl>
  static DriverBaseImpl* GetInstanceFromTokenForTesting(void* token) {
    fdf_internal::DriverServer2<DriverBaseImpl>* driver_server =
        static_cast<fdf_internal::DriverServer2<DriverBaseImpl>*>(token);
    return static_cast<DriverBaseImpl*>(driver_server->GetDriverBaseImpl());
  }

  explicit DriverBase2(std::string_view name);

  DriverBase2(const DriverBase2&) = delete;
  DriverBase2& operator=(const DriverBase2&) = delete;

  void DriverBaseInternalInit(DriverContext& context,
                              fdf::UnownedSynchronizedDispatcher driver_dispatcher);

  using InitMethodCallback =
      fit::callback<zx::result<>(async_dispatcher_t*, Namespace&, std::string_view)>;

  // Callbacks that are invoked prior to the start hook.
  void RegisterInitMethods(InitMethodCallback cb) {}

  // The destructor is called right after the |Stop| method.
  virtual ~DriverBase2();

  // This method will be called by the factory to start the driver. This is when
  // the driver should setup the outgoing directory through `outgoing()->Add...` calls.
  // Do not call Serve, as it has already been called by the |DriverBase2| constructor.
  // Child nodes can be created here synchronously or asynchronously as long as all of the
  // protocols being offered to the child has been added to the outgoing directory first.
  // There are two versions of this method which may be implemented depending on whether Start would
  // like to complete synchronously or asynchronously. The driver may override either one of these
  // methods, but must implement one. The asynchronous version will be called over the synchronous
  // version if both are implemented.
  virtual zx::result<> Start(DriverContext context) { return zx::error(ZX_ERR_NOT_SUPPORTED); }
  virtual void Start(DriverContext context, StartCompleter completer) {
    completer(Start(std::move(context)));
  }

  // This provides a way for the driver to asynchronously prepare to stop. The driver should
  // initiate any teardowns that need to happen on the driver dispatchers. Once it is ready to stop,
  // the completer's Complete function can be called (from any thread/context) with a result.
  // After the completer is called, the framework will shutdown all of the driver's fdf dispatchers
  // and deallocate the driver.
  virtual void Stop(StopCompleter completer) { completer(zx::ok()); }

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  // Optional suspend hook. Called when power_managed_dispatchers_enabled is set in the manifest's
  // program section. This hook is executed when the power element transitions to a suspended state,
  // after all pending tasks have run.
  virtual void SystemSuspend(SuspendCompleter completer) { completer(zx::ok()); }

  // Optional resume hook. Called when power_managed_dispatchers_enabled is set in the manifest's
  // program section. This hook is executed when the power element transitions to a running state or
  // a wake vector triggers, before any other tasks are executed. |pe_lease| contains the lease
  // associated with the wake if triggered by a wake vector.
  virtual void SystemResume(std::optional<fuchsia_power_broker::LeaseToken> pe_lease,
                            ResumeCompleter completer) {
    completer(zx::ok());
  }
#endif

  // This can be used to log in driver factories:
  // `driver->logger().log(fdf::INFO, "...");`
  Logger& logger() { return *logger_; }

  // Client to the `fuchsia.driver.framework/Node` protocol provided by the driver framework.
  // This can be used to add children to the node that the driver is bound to.
  fidl::ClientEnd<fuchsia_driver_framework::Node> take_node() {
    ZX_ASSERT(node_.is_valid());
    return std::move(node_);
  }

  const fidl::ClientEnd<fuchsia_driver_framework::Node>& node() const {
    ZX_ASSERT(node_.is_valid());
    return node_;
  }

  // Creates an owned child node on the node that the driver is bound to. The driver framework will
  // NOT try to match and bind a driver to this child as it is owned by the current driver.
  //
  // The |node()| must not have been moved out manually by the user. This is a synchronous call
  // and requires that the dispatcher allow sync calls.
  zx::result<OwnedChildNode> AddOwnedChild(std::string_view node_name);

  // Creates a child node with the given offers and properties on the node that the driver is
  // bound to. The driver framework will try to match and bind a driver to this child.
  //
  // The |node()| must not have been moved out manually by the user. This is a synchronous call
  // and requires that the dispatcher allow sync calls.
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
      std::string_view node_name,
      cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
      cpp20::span<const fuchsia_driver_framework::Offer> offers);

  // Creates an owned child node with devfs support on the node that the driver is bound to. The
  // driver framework will NOT try to match and bind a driver to this child as it is already owned
  // by the current driver.
  //
  // The |node()| must not have been moved out manually by the user. This is a synchronous call
  // and requires that the dispatcher allow sync calls.
  zx::result<OwnedChildNode> AddOwnedChild(std::string_view node_name,
                                           fuchsia_driver_framework::DevfsAddArgs& devfs_args);

  // Creates a child node with devfs support and the given offers and properties on the node that
  // the driver is bound to. The driver framework will try to match and bind a driver to this child.
  //
  // The |node()| must not have been moved out manually by the user. This is a synchronous call
  // and requires that the dispatcher allow sync calls.
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
      std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
      cpp20::span<const fuchsia_driver_framework::NodeProperty> properties,
      cpp20::span<const fuchsia_driver_framework::Offer> offers);

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
      std::string_view node_name,
      cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
      cpp20::span<const fuchsia_driver_framework::Offer> offers);

  zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
      std::string_view node_name, fuchsia_driver_framework::DevfsAddArgs& devfs_args,
      cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
      cpp20::span<const fuchsia_driver_framework::Offer> offers);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  // Provides a resource with the given offers and properties on the node that the driver is
  // bound to. Other nodes can depend on this resource.
  //
  // The |node()| must not have been moved out manually by the user. This is a synchronous call
  // and requires that the dispatcher allow sync calls.
  zx::result<fidl::ClientEnd<fuchsia_driver_framework::ResourceController>> ProvideResource(
      std::string_view node_name,
      cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
      cpp20::span<const fuchsia_driver_framework::Offer> offers);
#endif

 protected:
  friend DriverContext;

  // The logger can't be private because the logging macros rely on it.
  // NOLINTNEXTLINE(misc-non-private-member-variables-in-classes)
  std::unique_ptr<Logger> logger_;

  // The name of the driver that is given to the DriverBase2 constructor.
  std::string_view name() const { return name_; }

  // Used to access the outgoing directory that the driver is serving. Can be used to add both
  // zircon and driver transport outgoing services.
  std::shared_ptr<OutgoingDirectory>& outgoing() { return outgoing_; }

  // The unowned synchronized driver dispatcher that the driver is started with.
  const fdf::UnownedSynchronizedDispatcher& driver_dispatcher() const { return driver_dispatcher_; }

  // The async_dispatcher_t interface of the synchronized driver dispatcher that the driver
  // is started with.
  async_dispatcher_t* dispatcher() const { return driver_dispatcher_->async_dispatcher(); }

 private:
  std::string name_;
  fidl::ClientEnd<fuchsia_driver_framework::Node> node_;
  std::shared_ptr<OutgoingDirectory> outgoing_;
  fdf::UnownedSynchronizedDispatcher driver_dispatcher_;
};

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_DRIVER_BASE2_H_
