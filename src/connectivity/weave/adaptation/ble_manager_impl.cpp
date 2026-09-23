// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// clang-format off
#include <Weave/DeviceLayer/internal/WeaveDeviceLayerInternal.h>
#include <Weave/DeviceLayer/internal/BLEManager.h>
// clang-format on

#include <lib/syslog/cpp/macros.h>

#include "fuchsia/bluetooth/cpp/fidl.h"
#include "lib/fpromise/result.h"
#include "weave_inspector.h"

#if WEAVE_DEVICE_CONFIG_ENABLE_WOBLE

namespace gatt = fuchsia::bluetooth::gatt2;

namespace nl::Weave::DeviceLayer::Internal {
namespace {
using nl::Weave::WeaveInspector;

constexpr gatt::ServiceHandle kServiceHandle{1};
/// UUID of weave service obtained from SIG, in canonical 8-4-4-4-12 string format.
constexpr char kServiceUuid[] = "0000FEAF-0000-1000-8000-00805F9B34FB";

// Offsets into |kWeaveBleChars| for specific characteristic.
enum WeaveBleChar {
  // Weave service characteristic C1(write)
  kWeaveBleCharWrite = 0,
  // Weave service characteristic C2(indicate)
  kWeaveBleCharIndicate = 1,
};

// Type definitions for the Write & Indicate characteristics.
constexpr gatt::Handle kWeaveBleCharWriteHandle{WeaveBleChar::kWeaveBleCharWrite};
constexpr gatt::Handle kWeaveBleCharIndicateHandle{WeaveBleChar::kWeaveBleCharIndicate};

// An array that holds the UUID for each |WeaveBleChar|
constexpr WeaveBleUUID kWeaveBleChars[] = {
    // UUID for |kWeaveBleCharWrite|
    {{0x18, 0xEE, 0x2E, 0xF5, 0x26, 0x3D, 0x45, 0x59, 0x95, 0x9F, 0x4F, 0x9C, 0x42, 0x9F, 0x9D,
      0x11}},
    // UUID for |kWeaveBleCharIndicate|
    {{0x18, 0xEE, 0x2E, 0xF5, 0x26, 0x3D, 0x45, 0x59, 0x95, 0x9F, 0x4F, 0x9C, 0x42, 0x9F, 0x9D,
      0x12}}};

}  // unnamed namespace

BLEManagerImpl BLEManagerImpl::sInstance;

BLEManagerImpl::BLEManagerImpl() : woble_connection_(this), gatt_service_(this) {
  // BleLayer does not initialize this callback, set it NULL to avoid accessing location pointed by
  // garbage value when not set explicitly.
  OnWeaveBleConnectReceived = nullptr;
}

WEAVE_ERROR BLEManagerImpl::_Init() {
  WEAVE_ERROR err;
  auto svc = PlatformMgrImpl().GetComponentContextForProcess()->svc();

  FX_CHECK(svc->Connect(gatt_server_.NewRequest()) == ZX_OK)
      << "Failed to connect to " << gatt::Server::Name_;
  FX_CHECK(svc->Connect(peripheral_.NewRequest()) == ZX_OK)
      << "Failed to connect to " << fuchsia::bluetooth::le::Peripheral::Name_;

  adv_handle_.set_error_handler(
      [](zx_status_t status) { FX_LOGS(INFO) << "LE advertising was stopped: " << status; });

  if (ConfigurationMgrImpl().IsWoBLEEnabled()) {
    service_mode_ = ConnectivityManager::kWoBLEServiceMode_Enabled;
  }

  memset(device_name_, 0, sizeof(device_name_));

  flags_ = 0;
  if (ConfigurationMgrImpl().IsWoBLEAdvertisementEnabled()) {
    flags_ = kFlag_AdvertisingEnabled;
  }

  // Initialize the Weave BleLayer.
  err = BleLayer::Init(this, this, &SystemLayer);
  if (err != BLE_NO_ERROR) {
    FX_LOGS(ERROR) << "BLE Layer init failed: " << ErrorStr(err);
    return err;
  }

  PlatformMgr().ScheduleWork(DriveBLEState, reinterpret_cast<intptr_t>(this));
  return WEAVE_NO_ERROR;
}

WEAVE_ERROR BLEManagerImpl::_SetWoBLEServiceMode(WoBLEServiceMode service_mode) {
  if (service_mode == ConnectivityManager::kWoBLEServiceMode_NotSupported) {
    return WEAVE_ERROR_INVALID_ARGUMENT;
  }
  if (service_mode_ == ConnectivityManager::kWoBLEServiceMode_NotSupported) {
    return WEAVE_ERROR_UNSUPPORTED_WEAVE_FEATURE;
  }
  if (service_mode != service_mode_) {
    service_mode_ = service_mode;
    PlatformMgr().ScheduleWork(DriveBLEState, reinterpret_cast<intptr_t>(this));
  }
  return WEAVE_NO_ERROR;
}

WEAVE_ERROR BLEManagerImpl::_SetAdvertisingEnabled(bool advertising_enable) {
  if (service_mode_ == ConnectivityManager::kWoBLEServiceMode_NotSupported) {
    return WEAVE_ERROR_UNSUPPORTED_WEAVE_FEATURE;
  }

  if (GetFlag(flags_, kFlag_AdvertisingEnabled) != advertising_enable) {
    SetFlag(flags_, kFlag_AdvertisingEnabled, advertising_enable);
    PlatformMgr().ScheduleWork(DriveBLEState, reinterpret_cast<intptr_t>(this));
  }
  return WEAVE_NO_ERROR;
}

WEAVE_ERROR BLEManagerImpl::_SetFastAdvertisingEnabled(bool fast_advertising_enable) {
  if (service_mode_ == ConnectivityManager::kWoBLEServiceMode_NotSupported) {
    return WEAVE_ERROR_UNSUPPORTED_WEAVE_FEATURE;
  }
  // TODO(https://fxbug.dev/42130069): Enable bluetooth fast advertising when needed
  return WEAVE_NO_ERROR;
}

WEAVE_ERROR BLEManagerImpl::_GetDeviceName(char* device_name, size_t device_name_size) {
  if (strlen(device_name_) >= device_name_size) {
    return WEAVE_ERROR_BUFFER_TOO_SMALL;
  }
  strncpy(device_name, device_name_, device_name_size);
  return WEAVE_NO_ERROR;
}

WEAVE_ERROR BLEManagerImpl::_SetDeviceName(const char* device_name) {
  if (service_mode_ == ConnectivityManager::kWoBLEServiceMode_NotSupported) {
    return WEAVE_ERROR_UNSUPPORTED_WEAVE_FEATURE;
  }
  if (device_name != nullptr && device_name[0] != 0) {
    if (strlen(device_name) >= kMaxDeviceNameLength) {
      return WEAVE_ERROR_INVALID_ARGUMENT;
    }
    strcpy(device_name_, device_name);
    SetFlag(flags_, kFlag_UseCustomDeviceName);
  } else {
    device_name_[0] = 0;
    ClearFlag(flags_, kFlag_UseCustomDeviceName);
  }
  return WEAVE_NO_ERROR;
}

void BLEManagerImpl::_OnPlatformEvent(const WeaveDeviceEvent* event) {
  BLEManagerImpl* instance;
  WoBLEConState* connection_state;
  if (event == nullptr) {
    // Ignore null weave device event.
    return;
  }
  auto& inspector = WeaveInspector::GetWeaveInspector();
  switch (event->Type) {
    case DeviceEventType::kWoBLESubscribe:
      connection_state = static_cast<WoBLEConState*>(event->WoBLESubscribe.ConId);
      ZX_ASSERT_MSG(connection_state != nullptr,
                    "Received WoBLE subscribe event without connection state");
      instance = connection_state->instance;
      ZX_ASSERT_MSG(instance != nullptr, "Received WoBLE subscribe event with NULL instance");
      instance->HandleSubscribeReceived(event->WoBLESubscribe.ConId, &WEAVE_BLE_SVC_ID,
                                        &kWeaveBleChars[WeaveBleChar::kWeaveBleCharIndicate]);

      // Post a WoBLEConnectionEstablished event to the DeviceLayer
      WeaveDeviceEvent connection_established_event;
      connection_established_event.Type = DeviceEventType::kWoBLEConnectionEstablished;
      PlatformMgr().PostEvent(&connection_established_event);
      inspector.NotifySetupStateChange(WeaveInspector::kSetupState_BLEConnected);
      break;

    case DeviceEventType::kWoBLEUnsubscribe:
      connection_state = static_cast<WoBLEConState*>(event->WoBLEUnsubscribe.ConId);
      ZX_ASSERT_MSG(connection_state != nullptr,
                    "Received WoBLE unsubscribe event without connection state");
      instance = connection_state->instance;
      ZX_ASSERT_MSG(instance != nullptr, "Received WoBLE unsubscribe event with NULL instance");
      instance->HandleUnsubscribeReceived(event->WoBLEUnsubscribe.ConId, &WEAVE_BLE_SVC_ID,
                                          &kWeaveBleChars[WeaveBleChar::kWeaveBleCharIndicate]);
      inspector.NotifySetupStateChange(WeaveInspector::kSetupState_Initialized);
      break;
    case DeviceEventType::kWoBLEWriteReceived:
      connection_state = static_cast<WoBLEConState*>(event->WoBLEWriteReceived.ConId);
      ZX_ASSERT_MSG(connection_state != nullptr,
                    "Received WoBLE write event without connection state");
      instance = connection_state->instance;
      ZX_ASSERT_MSG(instance != nullptr, "Received WoBLE write event with NULL instance");
      instance->HandleWriteReceived(event->WoBLEWriteReceived.ConId, &WEAVE_BLE_SVC_ID,
                                    &kWeaveBleChars[WeaveBleChar::kWeaveBleCharWrite],
                                    event->WoBLEWriteReceived.Data);
      break;
    case DeviceEventType::kWoBLEIndicateConfirm:
      connection_state = static_cast<WoBLEConState*>(event->WoBLEIndicateConfirm.ConId);
      ZX_ASSERT_MSG(connection_state != nullptr,
                    "Received WoBLE indication confirmation event without connection state");
      instance = connection_state->instance;
      ZX_ASSERT_MSG(instance != nullptr,
                    "Received WoBLE indication confirmation event with NULL instance");
      instance->HandleIndicationConfirmation(event->WoBLEIndicateConfirm.ConId, &WEAVE_BLE_SVC_ID,
                                             &kWeaveBleChars[WeaveBleChar::kWeaveBleCharIndicate]);
      break;
    case DeviceEventType::kWoBLEConnectionError:
      connection_state = static_cast<WoBLEConState*>(event->WoBLEConnectionError.ConId);
      ZX_ASSERT(connection_state != nullptr);
      instance = connection_state->instance;
      ZX_ASSERT(instance != nullptr);
      instance->HandleConnectionError(event->WoBLEConnectionError.ConId,
                                      event->WoBLEConnectionError.Reason);
      break;
    default:
      // Ignore events not intended for BLEManager.
      break;
  }
}

bool BLEManagerImpl::SendIndication(BLE_CONNECTION_OBJECT conId, const WeaveBleUUID* svcId,
                                    const WeaveBleUUID* charId, PacketBuffer* data) {
  FX_LOGS(INFO) << "SendIndication request (service id = " << svcId << ", char id = " << charId
                << ")";
  auto connection_state = static_cast<WoBLEConState*>(conId);
  if (!connection_state) {
    PacketBuffer::Free(data);
    return false;
  }

  std::vector<uint8_t> value(data->DataLength());
  std::copy(data->Start(), data->Start() + data->DataLength(), value.begin());

  gatt::ValueChangedParameters notification_value;
  notification_value.set_handle(kWeaveBleCharIndicateHandle);
  std::vector<fuchsia::bluetooth::PeerId> peer_ids = {connection_state->peer_id};
  notification_value.set_peer_ids(std::move(peer_ids));
  notification_value.set_value(std::move(value));
  zx::eventpair confirm_ours;
  zx::eventpair confirm_theirs;
  zx::eventpair::create(/*options=*/0, &confirm_ours, &confirm_theirs);
  gatt_service_.events().OnIndicateValue(std::move(notification_value), std::move(confirm_theirs));

  // Save a reference to the buffer until we get a indication for the notification.
  connection_state->pending_ind_buf = data;
  data = nullptr;

  // TODO(https://fxbug.dev/42130443, https://fxbug.dev/42131435): The peer confirmation currently isn't returned to the
  // caller. Proceed as if the confirmation is received, to avoid closing the connection. When the
  // bug is fixed, block until the confirmation is received and handle it.
  PacketBuffer::Free(connection_state->pending_ind_buf);
  // If the confirmation was successful...
  // Post an event to the Weave queue to process the indicate confirmation.
  WeaveDeviceEvent event;
  event.Type = DeviceEventType::kWoBLEIndicateConfirm;
  event.WoBLESubscribe.ConId = conId;
  PlatformMgr().PostEvent(&event);

  return true;
}

void BLEManagerImpl::DriveBLEState() {
  if (service_mode_ != ConnectivityManager::kWoBLEServiceMode_Enabled) {
    if (GetFlag(flags_, kFlag_Advertising)) {
      adv_handle_.Unbind();
      ClearFlag(flags_, kFlag_Advertising);
    }
    if (GetFlag(flags_, kFlag_GATTServicePublished)) {
      gatt_service_.Close(ZX_ERR_PEER_CLOSED);
      ClearFlag(flags_, kFlag_GATTServicePublished);
    }
    return;
  }

  if (!GetFlag(flags_, kFlag_GATTServicePublished)) {
    gatt::ServiceInfo gatt_service_info;

    gatt::Characteristic weave_characteristic_c1;
    weave_characteristic_c1.set_handle(kWeaveBleCharWriteHandle);
    fuchsia::bluetooth::Uuid c1_uuid;
    std::copy(std::rbegin(kWeaveBleChars[WeaveBleChar::kWeaveBleCharWrite].bytes),
              std::rend(kWeaveBleChars[WeaveBleChar::kWeaveBleCharWrite].bytes),
              std::begin(c1_uuid.value));
    weave_characteristic_c1.set_type(c1_uuid);
    weave_characteristic_c1.set_properties(gatt::CharacteristicPropertyBits::WRITE);
    weave_characteristic_c1.mutable_permissions()->mutable_write();

    gatt::Characteristic weave_characteristic_c2;
    weave_characteristic_c2.set_handle(kWeaveBleCharIndicateHandle);
    fuchsia::bluetooth::Uuid c2_uuid;
    std::copy(std::rbegin(kWeaveBleChars[WeaveBleChar::kWeaveBleCharIndicate].bytes),
              std::rend(kWeaveBleChars[WeaveBleChar::kWeaveBleCharIndicate].bytes),
              std::begin(c2_uuid.value));
    weave_characteristic_c2.set_type(c2_uuid);
    weave_characteristic_c2.set_properties(gatt::CharacteristicPropertyBits::READ |
                                           gatt::CharacteristicPropertyBits::INDICATE);
    weave_characteristic_c2.mutable_permissions()->mutable_read();
    weave_characteristic_c2.mutable_permissions()->mutable_update();

    std::vector<gatt::Characteristic> characteristics;
    characteristics.push_back(std::move(weave_characteristic_c1));
    characteristics.push_back(std::move(weave_characteristic_c2));

    gatt_service_info.set_handle(kServiceHandle);
    gatt_service_info.set_kind(gatt::ServiceKind::PRIMARY);
    fuchsia::bluetooth::Uuid uuid;
    std::copy(std::rbegin(WEAVE_BLE_SVC_ID.bytes), std::rend(WEAVE_BLE_SVC_ID.bytes),
              std::begin(uuid.value));
    gatt_service_info.set_type(uuid);
    gatt_service_info.set_characteristics(std::move(characteristics));

    gatt::Server_PublishService_Result result;
    gatt_server_->PublishService(std::move(gatt_service_info), gatt_service_.NewBinding(), &result);

    if (result.is_err()) {
      FX_LOGS(ERROR) << "Failed to publish GATT service for Weave. Error: " << result.err()
                     << ". Disabling WoBLE service";
      service_mode_ = ConnectivityManager::kWoBLEServiceMode_Disabled;
      return;
    }

    FX_LOGS(INFO) << "Published GATT service for Weave with UUID: " << kServiceUuid;
    SetFlag(flags_, kFlag_GATTServicePublished);
  }

  if (GetFlag(flags_, kFlag_AdvertisingEnabled) && !GetFlag(flags_, kFlag_Advertising)) {
    fuchsia::bluetooth::le::AdvertisingData advertising_data;
    fuchsia::bluetooth::le::AdvertisingParameters advertising_parameters;
    fuchsia::bluetooth::le::Peripheral_StartAdvertising_Result advertising_result;
    fuchsia::bluetooth::Uuid uuid;
    std::copy(std::rbegin(WEAVE_BLE_SVC_ID.bytes), std::rend(WEAVE_BLE_SVC_ID.bytes),
              std::begin(uuid.value));

    // If a custom device name has not been specified, generate a name based on the
    // configured prefix and bottom digits of the Weave device id.
    if (!GetFlag(flags_, kFlag_UseCustomDeviceName)) {
      size_t out_len;
      char device_name_prefix[kMaxDeviceNameLength - 3] = "";
      WEAVE_ERROR err = ConfigurationMgrImpl().GetBleDeviceNamePrefix(
          device_name_prefix, kMaxDeviceNameLength - 4, &out_len);
      if (err != WEAVE_NO_ERROR) {
        FX_LOGS(ERROR) << "Failed to get BLE device name prefix: " << err;
      }
      snprintf(device_name_, sizeof(device_name_), "%s%04" PRIX32, device_name_prefix,
               static_cast<uint32_t>(FabricState.LocalNodeId));
      device_name_[kMaxDeviceNameLength] = 0;
    }
    advertising_data.set_name(device_name_);
    advertising_data.set_service_uuids({{uuid}});

    advertising_parameters.set_connectable(true);
    advertising_parameters.set_data(std::move(advertising_data));
    advertising_parameters.set_mode_hint(fuchsia::bluetooth::le::AdvertisingModeHint::SLOW);

    peripheral_->StartAdvertising(std::move(advertising_parameters), adv_handle_.NewRequest(),
                                  &advertising_result);

    if (advertising_result.is_err()) {
      FX_LOGS(ERROR) << "Failed to advertise WoBLE service, disabling WoBLE: "
                     << static_cast<uint32_t>(advertising_result.err());
      service_mode_ = ConnectivityManager::kWoBLEServiceMode_Disabled;
      return;
    }
    FX_LOGS(INFO) << "Advertising Weave service for device: " << device_name_;
    SetFlag(flags_, kFlag_Advertising);
  }

  if (!GetFlag(flags_, kFlag_AdvertisingEnabled) && GetFlag(flags_, kFlag_Advertising)) {
    adv_handle_.Unbind();
    ClearFlag(flags_, kFlag_Advertising);
  }
}

void BLEManagerImpl::DriveBLEState(intptr_t arg) {
  auto instance = reinterpret_cast<BLEManagerImpl*>(arg);
  if (!instance) {
    FX_LOGS(ERROR) << "DriveBLEState called with NULL";
    return;
  }
  instance->DriveBLEState();
}

void BLEManagerImpl::CharacteristicConfiguration(fuchsia::bluetooth::PeerId peer_id,
                                                 gatt::Handle handle, bool notify, bool indicate,
                                                 CharacteristicConfigurationCallback callback) {
  FX_LOGS(INFO) << "CharacteristicConfiguration on peer " << peer_id.value << " (notify: " << notify
                << ", indicate: " << indicate << ") characteristic id " << handle.value;

  // Post an event to the Weave queue to process either a WoBLE Subscribe or Unsubscribe based on
  // whether the client is enabling or disabling indications.
  WeaveDeviceEvent event;
  event.Type = (indicate) ? DeviceEventType::kWoBLESubscribe : DeviceEventType::kWoBLEUnsubscribe;
  woble_connection_.peer_id = peer_id;
  event.WoBLESubscribe.ConId = static_cast<void*>(&woble_connection_);
  PlatformMgr().PostEvent(&event);
}

void BLEManagerImpl::ReadValue(fuchsia::bluetooth::PeerId peer_id, gatt::Handle handle,
                               int32_t offset, ReadValueCallback callback) {
  callback(fpromise::error(gatt::Error::READ_NOT_PERMITTED));
}

void BLEManagerImpl::WriteValue(gatt::LocalServiceWriteValueRequest request,
                                WriteValueCallback callback) {
  PacketBuffer* buf = nullptr;

  if (!request.has_peer_id() || !request.has_handle() || !request.has_offset() ||
      !request.has_value()) {
    FX_LOGS(WARNING) << "Write request is missing mandatory arguments. Ignoring.";
    callback(fpromise::error(gatt::Error::INVALID_PARAMETERS));
    return;
  }

  FX_LOGS(INFO) << "Write request received for peer " << request.peer_id().value
                << " at characteristic id " << request.handle().value << " at offset "
                << request.offset() << " length " << request.value().size();
  if (request.handle().value != kWeaveBleCharWriteHandle.value) {
    FX_LOGS(WARNING) << "Ignoring writes to characteristic other than weave characteristic "
                        "C1(write). Expected characteristic: "
                     << kWeaveBleCharWrite;
    callback(fpromise::error(gatt::Error::WRITE_NOT_PERMITTED));
    return;
  }

  if (request.offset() != 0) {
    FX_LOGS(ERROR) << "No offset expected on write to control point";
    callback(fpromise::error(gatt::Error::INVALID_OFFSET));
    return;
  }

  // Copy the data to a PacketBuffer.
  buf = PacketBuffer::New(0);
  if (!buf || buf->AvailableDataLength() < request.value().size()) {
    FX_LOGS(ERROR) << "Buffer not available";
    callback(fpromise::error(gatt::Error::INVALID_ATTRIBUTE_VALUE_LENGTH));
    PacketBuffer::Free(buf);
    return;
  }

  std::copy(request.value().begin(), request.value().end(), buf->Start());
  buf->SetDataLength(request.value().size());

  // Post an event to the Weave queue to deliver the data into the Weave stack.
  WeaveDeviceEvent event;
  event.Type = DeviceEventType::kWoBLEWriteReceived;
  event.WoBLEWriteReceived.ConId = static_cast<void*>(&woble_connection_);
  event.WoBLEWriteReceived.Data = buf;
  PlatformMgr().PostEvent(&event);
  buf = nullptr;

  callback(fpromise::ok());
}

}  // namespace nl::Weave::DeviceLayer::Internal

#endif  // WEAVE_DEVICE_CONFIG_ENABLE_WOBLE
