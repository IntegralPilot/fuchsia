# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types used by Bluetooth affordance."""

import dataclasses
import enum

import fidl_fuchsia_bluetooth as f_bt


class Implementation(enum.StrEnum):
    """Different Bluetooth affordance implementations available."""

    # Use Bluetooth affordances that is implemented using Fuchsia-Controller
    FUCHSIA_CONTROLLER = "fuchsia-controller"

    # Use Bluetooth affordances that is implemented using SL4F
    SL4F = "sl4f"


class BluetoothConnectionType(enum.Enum):
    """Transport type of Bluetooth pair and/or connections."""

    CLASSIC = 1
    LOW_ENERGY = 2


class BluetoothAcceptPairing(enum.StrEnum):
    """Pairing modes for Bluetooth Accept Pairing."""

    DEFAULT_INPUT_MODE = "NONE"
    DEFAULT_OUTPUT_MODE = "NONE"


class BluetoothAvrcpCommand(enum.StrEnum):
    """Commands that the AVRCP service can send."""

    PLAY = "Play"
    PAUSE = "Pause"


class BluetoothLEAppearance(enum.IntEnum):
    """Possible values for LE Appearance property to describe the external appearance of a peripheral at a high level."""

    UNKNOWN = 0
    PHONE = 64
    COMPUTER = 128
    WATCH = 192
    WATCH_SPORTS = 193
    CLOCK = 256
    DISPLAY = 320
    REMOTE_CONTROL = 384
    EYE_GLASSES = 448
    TAG = 512
    KEYRING = 576
    MEDIA_PLAYER = 640
    BARCODE_SCANNER = 704
    THERMOMETER = 768
    THERMOMETER_EAR = 769
    HEART_RATE_SENSOR = 832
    HEART_RATE_SENSOR_BELT = 833
    BLOOD_PRESSURE = 896
    BLOOD_PRESSURE_ARM = 897
    BLOOD_PRESSURE_WRIST = 898
    HID = 960
    HID_KEYBOARD = 961
    HID_MOUSE = 962
    HID_JOYSTICK = 963
    HID_GAMEPAD = 964
    HID_DIGITIZER_TABLET = 965
    HID_CARD_READER = 966
    HID_DIGITAL_PEN = 967
    HID_BARCODE_SCANNER = 968
    GLUCOSE_METER = 1024
    RUNNING_WALKING_SENSOR = 1088
    RUNNING_WALKING_SENSOR_IN_SHOE = 1089
    RUNNING_WALKING_SENSOR_ON_SHOE = 1090
    RUNNING_WALKING_SENSOR_ON_HIP = 1091
    CYCLING = 1152
    CYCLING_COMPUTER = 1153
    CYCLING_SPEED_SENSOR = 1154
    CYCLING_CADENCE_SENSOR = 1155
    CYCLING_POWER_SENSOR = 1156
    CYCLING_SPEED_AND_CADENCE_SENSOR = 1157
    PULSE_OXIMETER = 3136
    PULSE_OXIMETER_FINGERTIP = 3137
    PULSE_OXIMETER_WRIST = 3138
    WEIGHT_SCALE = 3200
    PERSONAL_MOBILITY = 3264
    PERSONAL_MOBILITY_WHEELCHAIR = 3265
    PERSONAL_MOBILITY_SCOOTER = 3266
    GLUCOSE_MONITOR = 3328
    SPORTS_ACTIVITY = 5184
    SPORTS_ACTIVITY_LOCATION_DISPLAY = 5185
    SPORTS_ACTIVITY_LOCATION_AND_NAV_DISPLAY = 5186
    SPORTS_ACTIVITY_LOCATION_POD = 5187
    SPORTS_ACTIVITY_LOCATION_AND_NAV_POD = 5188


@dataclasses.dataclass
class BluetoothPeerInfo:
    id: f_bt.PeerId
    address: list[int]
    connected: bool
    bonded: bool
    name: str | None = None
    appearance: int | None = None
    rssi: int | None = None
    services: list[f_bt.Uuid] | None = None
    technology: int | None = None
    tx_power: int | None = None
