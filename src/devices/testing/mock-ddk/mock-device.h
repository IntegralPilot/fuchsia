// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTING_MOCK_DDK_MOCK_DEVICE_H_
#define SRC_DEVICES_TESTING_MOCK_DDK_MOCK_DEVICE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/binding_priv.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fidl/cpp/wire/wire_messaging.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/span.h>
#include <lib/sync/completion.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <list>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

// Allow redefining the zx_device_t struct. MockDevice cannot be in the mock_ddk namespace.
#define MockDevice zx_device

namespace mock_ddk {
// Generic protocol.
struct Protocol {
  const void* ops;
  void* ctx;
};

struct ProtocolEntry {
  uint32_t id;
  Protocol proto;
};

using AnyHandler = fit::function<zx_status_t(zx::channel)>;

template <typename Protocol>
using TypedHandler = fit::function<void(fidl::internal::ServerEndType<Protocol> request)>;

// The driver runtime for all mock devices is shared.
// This is because there can only be one DriverRuntime,
// but users can create multiple root MockDevices.
std::shared_ptr<fdf_testing::DriverRuntime> GetDriverRuntime();

}  // namespace mock_ddk

// MockDevice is an implementation of the opaque type zx_device which mocks much of
// device host functionality.
// Mock devices are created in one of two ways:
// 1) Calling FakeRootParent.
// 2) Calling device_add_from_driver (which may be done indirectly by DdkAdd)
//
// Since device_add_from_driver takes zx_device_t* parent as an argument, FakeRootParent()
// must be called before adding any other devices.
// MockDevice does not use any global variables, so multiple fake parents can exist without
// interfering with each other.
// The FakeRootParent is limited in functionality, but protocols and metadata can be added to it
// to facilitate a child device's needs.
//
// The fake root parent is also important because it controls the lifecycle of its descendants.
// Unlike the device host, the parent of each MockDevice holds the MockDevice's refptr.
// This allows drivers to "leak" the pointer to the device like normal during bind.
// However, it means that tests using MockDevices must retain the reference to the fake root parent,
// or all the descendent devices will be deleted, because
// the root parent will recursively release all of its children upon destruction.
// When this happens, the release() op will be called on the device, allowing it to delete
// any context it created.
//
// Importantly, this is a mock implementation, not a fake. Any libDriver calls will be
// recorded, but the mock will not take any automatic action. This may result in unexpected
// behavior during device initialization and removal.
// Initialization:
//  The init() op is not automatically called on newly added devices. It can be manually called
//  by calling MockDevice::InitOp().
// Removal:
//  When attempting to remove dynamically devices, device_async_remove will not result in
//  device removal.
//  To process device removal, a helper function is provided below:
//  ReleaseFlaggedDevices recursively searches the device tree and calls unbind and release
//  on any device that has had device_async_remove called on it.
// Calling ReleaseOp() on a MockDevice will also cause it to be deleted.
//
// Things that this MockDevice does not handle (yet)
// Rebinding
// Composite devices
// Fidl messages
// Any automatic responses from the DDK
// This needs to be a struct, not a class, to match the public definition
struct MockDevice : public std::enable_shared_from_this<MockDevice> {
 public:
  // Create a Root Parent.  This device has limited functionality.
  // This will create the |fdf_testing::DriverRuntime| instance for the user. The instance will
  // start the driver runtime, and create a foreground driver dispatcher for the user.
  //
  // Background dispatchers can be created through this object. Grab the instance using the
  // |fdf_testing::DriverRuntime::GetInstance()| call.
  //
  // The test thread and the driver dispatcher thread are the same when using this. All driver
  // 'op' hooks are therefore run directly on the main test thread.
  static std::shared_ptr<MockDevice> FakeRootParent() {
    // Using `new` to access a non-public constructor.
    return std::shared_ptr<MockDevice>(new MockDevice(mock_ddk::GetDriverRuntime()));
  }

  ~MockDevice();

  // Calls for tracking libdriver calls made that reference this device:
  // The Register calls below create 4 functions each:
  // REGISTER_CALL_TRACKER( InitReply )  creates:
  //   zx_status_t WaitUntilInitReplyCalled();  <-- Blocking wait until InitReply is called
  //   void RecordInitReply();                  <-- Records the InitReply call
  //   bool InitReplyCalled();                  <-- Returns true if InitReply has been called.
  //   zx_status_t InitReplyCallStatus();       <-- Returns the status that was passed to InitReply
  //
  // The WaitUntil* functions are useful if you expect the reply/remove/etc to be called
  // in a different thread.
#define REGISTER_CALL_TRACKER(varname)                                                \
 private:                                                                             \
  bool varname##_call_made_ = false;                                                  \
  zx_status_t varname##_call_status_ = ZX_ERR_BAD_STATE;                              \
  sync_completion_t varname##_call_made_sync_;                                        \
                                                                                      \
 public:                                                                              \
  void Record##varname(zx_status_t status) {                                          \
    varname##_call_status_ = status;                                                  \
    varname##_call_made_ = true;                                                      \
    sync_completion_signal(&varname##_call_made_sync_);                               \
  }                                                                                   \
  zx_status_t WaitUntil##varname##Called(zx::time deadline = zx::time::infinite()) {  \
    return sync_completion_wait_deadline(&varname##_call_made_sync_, deadline.get()); \
  }                                                                                   \
  bool varname##Called() { return varname##_call_made_; }                             \
  zx_status_t varname##CallStatus() { return varname##_call_status_; }

  REGISTER_CALL_TRACKER(InitReply)
  REGISTER_CALL_TRACKER(AsyncRemove)
  REGISTER_CALL_TRACKER(UnbindReply)
  REGISTER_CALL_TRACKER(SuspendReply)
  REGISTER_CALL_TRACKER(Remove)
  REGISTER_CALL_TRACKER(ChildPreRelease)
#undef REGISTER_CALL_TRACKER

  // Functions for calling into the driver.
  // These are functions that the DDK normally calls, but are exposed here for testing purposes.
  // They should always be called from the main test thread.
  void InitOp();
  void UnbindOp();
  void ReleaseOp();
  void SuspendNewOp(uint8_t requested_state, bool enable_wake, uint8_t suspend_reason);
  bool MessageOp(fidl::IncomingHeaderAndMessage msg, device_fidl_txn_t txn);
  void ChildPreReleaseOp(void* child_ctx);
  bool HasUnbindOp() { return ops_->unbind != nullptr; }

  cpp20::span<const zx_device_str_prop_t> GetStringProperties() const { return str_props_; }

  const zx::vmo& GetInspectVmo() const { return inspect_; }

  fidl::UnownedClientEnd<fuchsia_io::Directory> outgoing() const { return outgoing_; }

  // Metadata is often set for the parent of a device, to be available when the device
  // calls device_get_metadata
  void SetMetadata(uint32_t type, const void* data, size_t data_length);

  // Structured configuration VMO is often set for the parent of a device, to be available when the
  // device calls device_get_config_vmo
  void SetConfigVmo(zx::vmo config_vmo);

  // device_get_protocol is usually called by child devices to get their parent protocols.
  // You can add protocols here to your device or your parent device.
  // if you want to add a protocol to a fragment, add the fragment's name as 'name'.
  void AddProtocol(uint32_t id, const void* ops, void* ctx, const char* name = "");

  // Add a protocol that is served by the namespace.
  // This is similar to AddProtocol, but the protocol is served by the namespace
  // instead of the device.
  template <typename Protocol>
  void AddNsProtocol(mock_ddk::TypedHandler<Protocol> handler,
                     const char* protocol_name = fidl::DiscoverableProtocolName<Protocol>) {
    auto bridge_func = [handler = std::move(handler)](zx::channel request) {
      fidl::ServerEnd<Protocol> server_end(std::move(request));
      handler(std::move(server_end));
      return ZX_OK;
    };
    AddNsProtocol(protocol_name, std::move(bridge_func));
  }

  // Add a protocol that is served by the namespace.
  // This is similar to AddProtocol, but the protocol is served by the namespace
  // instead of the device.
  void AddNsProtocol(const char* protocol_name, mock_ddk::AnyHandler callback);

  // You can add FIDL service here to your device or your parent device.
  // if you want to add a service to a fragment, add the fragment's name as 'name'.
  // Devices will use `device_connect_fragment_fidl_protocol` to connect to these protocols
  void AddFidlService(const char* service_name, fidl::ClientEnd<fuchsia_io::Directory> ns,
                      const char* name = "default");

  // This struct can also be a root parent device, with reduced functionality.
  // This allows the parent to store protocols that can be accessed by a child device.
  // If IsRootParent returns true, only the following calls may target this device:
  //  device_get_protocol
  //  device_get_metadata
  // These calls will be able to target this type of device soon:
  //  device_get_fragment
  bool IsRootParent() const { return parent_ == nullptr; }
  const char* name() const { return name_.c_str(); }

  size_t child_count() const { return children_.size(); }

  // Count all the descendants of this device.
  size_t descendant_count() const;

  std::list<std::shared_ptr<MockDevice>>& children() { return children_; }
  // Gets the child that was added to this parent most recently.
  // Returns nullptr if no children exist.
  MockDevice* GetLatestChild();

  // Access the Device class that created this MockDevice (for example, devices which
  // inherit from ddk::Device create a MockDevice when they call DdkAdd.)
  template <class DeviceType>
  DeviceType* GetDeviceContext() {
    return static_cast<DeviceType*>(ctx_);
  }

  // load_firmware support
  // First, set firmware by calling SetFirmware on a device.
  // Then, when the device calls load_firmware, a vmo will be created with the
  // firmware that was set.
  // SetFirmware can store any number of path:firmware pairs.
  // If path is empty, the firmware is stored in a way that will match any path
  // that has not also been provided to SetFirmware to store another firmware blob.
  void SetFirmware(std::vector<uint8_t> firmware, std::string_view path = {});
  // Convenience version which takes string:
  void SetFirmware(std::string firmware, std::string_view path = {});

 private:
  constexpr static zx_protocol_device_t kDefaultOps = {};
  // |ctx| must outlive |*out_dev|.  This is managed in the full binary by creating
  // the DriverHostContext in main() (having essentially a static lifetime).
  MockDevice(device_add_args_t* args, MockDevice* parent);

  // Calls make by the libdriver api:
  // device_add_from_driver calls Create:
  static zx_status_t Create(device_add_args_t* args, MockDevice* parent, MockDevice** out_dev);
  // Allow device_add_from_driver to access the Create function
  friend zx_status_t device_add_from_driver(zx_driver_t* drv, zx_device_t* parent,
                                            device_add_args_t* args, zx_device_t** out);

  // load_firmware_from_driver calls LoadFirmware:
  zx_status_t LoadFirmware(std::string_view path, zx_handle_t* fw, size_t* size);
  friend zx_status_t load_firmware_from_driver(zx_driver_t* drv, zx_device_t* device,
                                               const char* path, zx_handle_t* fw, size_t* size);

  // device_get_protocol and device_get_fragment_protocol call GetProtocol.
  // Get protocol can get the normal protocols for the device, if fragment_name = "".
  // Otherwise, it gets protocols associated with the fragment identified by fragment_name.
  zx_status_t GetProtocol(uint32_t proto_id, void* protocol, const char* fragment_name = "") const;
  friend zx_status_t device_get_protocol(const zx_device_t* device, uint32_t proto_id,
                                         void* protocol);
  friend zx_status_t device_get_fragment_protocol(zx_device_t* device, const char* fragment_name,
                                                  uint32_t proto_id, void* protocol);

  zx_status_t ConnectToFidlProtocol(const char* service_name, const char* protocol_name,
                                    zx::channel request, const char* fragment_name = "");
  friend zx_status_t device_connect_fragment_fidl_protocol(zx_device_t* device,
                                                           const char* fragment_name,
                                                           const char* service_name,
                                                           const char* protocol_name,
                                                           zx_handle_t request);

  zx_status_t ConnectToRuntimeProtocol(const char* service_name, const char* protocol_name,
                                       fdf::Channel request, const char* fragment_name = "");
  friend zx_status_t device_connect_fragment_runtime_protocol(zx_device_t* device,
                                                              const char* fragment_name,
                                                              const char* service_name,
                                                              const char* protocol_name,
                                                              fdf_handle_t request);

  zx_status_t ConnectToNsProtocol(const char* protocol_name, zx::channel request);
  friend zx_status_t device_connect_ns_protocol(zx_device_t* device, const char* protocol_name,
                                                zx_handle_t request);

  // device_get_metadata calls GetMetadata:
  zx_status_t GetMetadata(uint32_t type, void* buf, size_t buflen, size_t* actual);
  friend zx_status_t device_get_metadata(zx_device_t* device, uint32_t type, void* buf,
                                         size_t buflen, size_t* actual);

  // device_get_config_vmo calls GetConfigVmo:
  zx::vmo& GetConfigVmo() { return config_vmo_; }
  friend zx_status_t device_get_config_vmo(zx_device_t* device, zx_handle_t* config_vmo);

  // Default constructor for making deprecated root parent without dispatcher integration.
  // TODO(https://fxbug.dev/42075363): Remove when all tests migrated.
  explicit MockDevice() : driver_runtime_(nullptr) {}

  // Constructor for root parent.
  explicit MockDevice(std::shared_ptr<fdf_testing::DriverRuntime> runtime)
      : driver_runtime_(std::move(runtime)) {}

  // Copies this device's metadata to all descendants.
  void PropagateMetadata();

  // list of this device's children in the device tree
  std::list<std::shared_ptr<MockDevice>> children_;
  // Stores the normal protocols under the key "", fragment protocols under their name.
  std::unordered_map<std::string, std::list<mock_ddk::ProtocolEntry>> protocols_;
  // The first key is the first key is the fragment name and the second key is the protocol name. ""
  // is used as the key when there is no fragment.
  std::unordered_map<std::string, mock_ddk::AnyHandler> fidl_protocols_;
  // The first key is the first key is the fragment name and the second key is the service name. ""
  // is used as the key when there is no fragment.
  std::unordered_map<std::string,
                     std::unordered_map<std::string, fidl::ClientEnd<fuchsia_io::Directory>>>
      fidl_services_;

  std::unordered_map<std::string_view, std::vector<uint8_t>> firmware_;

  // Map of metadata set by SetMetadata.
  std::unordered_map<uint32_t, std::vector<uint8_t>> metadata_;

  // Set by SetConfigVmo.
  zx::vmo config_vmo_;

  // Map of variables set by SetVariable.
  std::unordered_map<std::string, std::string> variables_;

  // parent in the device tree
  MockDevice* parent_ = nullptr;  // This will default to a nullptr, for the root parent.

  const zx_protocol_device_t* ops_ = &kDefaultOps;
  // reserved for driver use; will not be touched by MockDevice
  void* ctx_ = nullptr;

  std::string name_;

  std::vector<zx_device_str_prop_t> str_props_;
  zx::vmo inspect_;
  fidl::ClientEnd<fuchsia_io::Directory> outgoing_;
  // Created as a refcounted global that's shared between all MockDevices.
  std::shared_ptr<fdf_testing::DriverRuntime> driver_runtime_;
};

namespace mock_ddk {
// Helper function:
// Performs the unbind and release of any device below the input device that
// has had device_async_remove called on it.
// returns an error if there was a problem waiting for UnbindReply to be called.
// This function will call unbind devices that are to be removed, and block
// until device_unbind_reply is called.
zx_status_t ReleaseFlaggedDevices(MockDevice* device, async_dispatcher_t* dispatcher = nullptr);

// Sets the global minimum severity for mock-ddk logging.
void SetMinLogSeverity(fx_log_severity_t severity);

}  // namespace mock_ddk

#endif  // SRC_DEVICES_TESTING_MOCK_DDK_MOCK_DEVICE_H_
