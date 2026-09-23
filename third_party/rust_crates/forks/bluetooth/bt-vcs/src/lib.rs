// Copyright 2024 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::packet_encoding::Encodable;
use bt_common::Uuid;
use bt_gatt::{
    client::{FromCharacteristic, PeerService, PeerServiceHandle},
    types::WriteMode,
    Characteristic, Client,
};

use futures::TryFutureExt;
use parking_lot::Mutex;
use std::sync::Arc;
use thiserror::Error;

pub const VCS_UUID: Uuid = Uuid::from_u16(0x1844);

pub mod debug;

/// Volume State Characteristic
/// See VCS v1.0 Section 2.3.1
#[derive(Debug, Clone)]
pub struct VolumeState {
    handle: bt_gatt::types::Handle,
    /// Unitless value, step sizes are implementation-specific
    setting: u8,
    /// True if the audio is muted.  Does not affect
    /// [`setting`](VolumeState::setting).
    mute: bool,
    /// Server-incremented counter, used to invalidate commands against stale
    /// state. Wraps around from 255 to 0.
    change_counter: u8,
}

impl VolumeState {
    fn from_value(
        handle: bt_gatt::types::Handle,
        value: &[u8],
    ) -> core::result::Result<Self, bt_common::packet_encoding::Error> {
        let mut val = Self { handle, setting: 0, mute: false, change_counter: 0 };
        val.update_from_value(value)?;
        Ok(val)
    }

    fn update_from_value(
        &mut self,
        value: &[u8],
    ) -> core::result::Result<(), bt_common::packet_encoding::Error> {
        if value.len() < 3 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        self.setting = value[0];
        self.mute = match value[1] {
            0 => false,
            1 => true,
            _ => return Err(bt_common::packet_encoding::Error::OutOfRange),
        };
        self.change_counter = value[2];
        Ok(())
    }
}

impl FromCharacteristic for VolumeState {
    const UUID: Uuid = Uuid::from_u16(0x2B7D);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> core::result::Result<Self, bt_common::packet_encoding::Error> {
        Self::from_value(characteristic.handle, value)
    }

    fn update(
        &mut self,
        new_value: &[u8],
    ) -> core::result::Result<&mut Self, bt_common::packet_encoding::Error> {
        self.update_from_value(new_value)?;
        Ok(self)
    }
}

/// Volume Control Point
/// See VCS v1.0 Section 3.2
#[derive(Debug, Clone)]
pub struct VolumeControlPoint {
    handle: bt_gatt::types::Handle,
}

impl VolumeControlPoint {
    const UUID: Uuid = Uuid::from_u16(0x2B7E);
}

#[derive(Debug, Clone, PartialEq)]
pub enum VcpProcedure {
    RelativeVolumeDown,
    RelativeVolumeUp,
    UnmuteRelativeVolumeDown,
    UnmuteRelativeVolumeUp,
    SetAbsoluteVolume { setting: u8 },
    Unmute,
    Mute,
}

impl VcpProcedure {
    fn opcode(&self) -> u8 {
        match self {
            VcpProcedure::RelativeVolumeDown => 0x00,
            VcpProcedure::RelativeVolumeUp => 0x01,
            VcpProcedure::UnmuteRelativeVolumeDown => 0x02,
            VcpProcedure::UnmuteRelativeVolumeUp => 0x03,
            VcpProcedure::SetAbsoluteVolume { .. } => 0x04,
            VcpProcedure::Unmute => 0x05,
            VcpProcedure::Mute => 0x06,
        }
    }
}

pub struct VolumeControlPointOperation {
    procedure: VcpProcedure,
    change_counter: u8,
}

impl Encodable for VolumeControlPointOperation {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> core::primitive::usize {
        if let VcpProcedure::SetAbsoluteVolume { .. } = self.procedure {
            return 3;
        }
        // All the other operations only have a change counter parameter
        2
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(bt_common::packet_encoding::Error::BufferTooSmall);
        }

        buf[0] = self.procedure.opcode();
        buf[1] = self.change_counter;
        if let VcpProcedure::SetAbsoluteVolume { setting } = self.procedure {
            buf[2] = setting;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VolumeSettingPersisted {
    Reset,
    UserSet,
}

impl TryFrom<&[u8]> for VolumeSettingPersisted {
    type Error = bt_common::packet_encoding::Error;

    fn try_from(value: &[u8]) -> std::result::Result<Self, Self::Error> {
        if value.len() < 1 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        if (value[0] & 0x01) != 0 {
            Ok(VolumeSettingPersisted::UserSet)
        } else {
            Ok(VolumeSettingPersisted::Reset)
        }
    }
}

/// Volume Flags characteristic
/// See VCS v1.0 Section 3.3
#[derive(Debug, Clone)]
pub struct VolumeFlags {
    handle: bt_gatt::types::Handle,
    persisted: VolumeSettingPersisted,
}

impl FromCharacteristic for VolumeFlags {
    const UUID: Uuid = Uuid::from_u16(0x2B7F);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> core::result::Result<Self, bt_common::packet_encoding::Error> {
        let persisted = VolumeSettingPersisted::try_from(value)?;
        Ok(Self { handle: characteristic.handle, persisted })
    }

    fn update(
        &mut self,
        new_value: &[u8],
    ) -> core::result::Result<&mut Self, bt_common::packet_encoding::Error> {
        self.persisted = VolumeSettingPersisted::try_from(new_value)?;
        Ok(self)
    }
}

pub struct VolumeControlClient<T: bt_gatt::GattTypes> {
    service: T::PeerService,
    state: Arc<Mutex<VolumeState>>,
    control_point: VolumeControlPoint,
    flags: Arc<Mutex<VolumeFlags>>,
}

impl<T: bt_gatt::GattTypes> std::fmt::Display for VolumeControlClient<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let VolumeState { change_counter, setting, mute, .. } = self.state.lock().clone();
        let VolumeFlags { persisted, .. } = self.flags.lock().clone();
        let mute_str = mute.then_some("MUTED ").unwrap_or("");
        write!(
            f,
            "Volume Control @ {change_counter}: Current Volume: {setting} {mute_str}from {persisted:?}"
        )
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Issue encoding / decoding: {0}")]
    Decoding(#[from] bt_common::packet_encoding::Error),
    #[error("GATT error: {0}")]
    Gatt(bt_gatt::types::Error),
    #[error("Required Characteristic was not found: {0}")]
    RequiredCharNotFound(&'static str),
    #[error("Change Counter Mismatch")]
    ChangeCounterMismatch,
    #[error("Opcode Not Supported")]
    OpcodeNotSupported,
}

impl From<bt_gatt::types::Error> for Error {
    fn from(value: bt_gatt::types::Error) -> Self {
        use bt_gatt::types::GattError::*;
        match value {
            bt_gatt::types::Error::Gatt(ApplicationError80) => Self::ChangeCounterMismatch,
            bt_gatt::types::Error::Gatt(ApplicationError81) => Self::OpcodeNotSupported,
            other => Self::Gatt(other),
        }
    }
}

// TODO(b/441327871): Add notification method for volume state changes.
impl<T: bt_gatt::GattTypes> VolumeControlClient<T> {
    pub async fn from_service(service: T::PeerService) -> Result<Self, Error> {
        let mut state = None;
        let mut control_point = None;
        let mut flags = None;
        for chr in service.discover_characteristics(None).await? {
            if chr.uuid == VolumeState::UUID {
                let _ = state.insert(VolumeState::try_read::<T>(chr, &service).await?);
            } else if chr.uuid == VolumeControlPoint::UUID {
                let _ = control_point.insert(VolumeControlPoint { handle: chr.handle });
            } else if chr.uuid == VolumeFlags::UUID {
                let _ = flags.insert(VolumeFlags::try_read::<T>(chr, &service).await?);
            }
        }

        if state.is_none() {
            return Err(Error::RequiredCharNotFound("Volume State"));
        }
        if control_point.is_none() {
            return Err(Error::RequiredCharNotFound("Volume Conrol Point"));
        }
        if flags.is_none() {
            return Err(Error::RequiredCharNotFound("Volume Flags"));
        }

        Ok(Self {
            service,
            state: Arc::new(Mutex::new(state.unwrap())),
            control_point: control_point.unwrap(),
            flags: Arc::new(Mutex::new(flags.unwrap())),
        })
    }

    pub async fn connect(client: &T::Client) -> Result<Option<Self>, Error> {
        let handles = client.find_service(VCS_UUID).await?;
        let Some(handle) = handles.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(handle.connect().map_err(Into::into).and_then(Self::from_service).await?))
    }

    fn change_counter(&self) -> u8 {
        self.state.lock().change_counter
    }

    // Update the state of the volume by reading the characteristics.
    // Returns the new volume level if successful, and an Error otherwise.
    pub async fn update(&self) -> Result<u8, Error> {
        let state_handle = self.state.lock().handle;

        let mut state_buf = [0; 3];
        let _ = self.service.read_characteristic(&state_handle, 0, &mut state_buf).await?;
        let setting = self.state.lock().update(&state_buf)?.setting;

        let flags_handle = self.flags.lock().handle;
        let mut flags_buf = [0; 1];
        let _ = self.service.read_characteristic(&flags_handle, 0, &mut flags_buf).await?;
        let _ = self.flags.lock().update(&flags_buf)?;

        Ok(setting)
    }

    async fn send_control_pt(&self, procedure: VcpProcedure) -> Result<(), Error> {
        let change_counter = self.change_counter();
        let op = VolumeControlPointOperation { procedure, change_counter };
        let mut buf = [0; 2];
        op.encode(&mut buf)?;
        self.service
            .write_characteristic(&self.control_point.handle, WriteMode::None, 0, &buf)
            .await
            .map_err(Into::into)
    }

    /// Relative volume up.  Should increase the volume by a static step size
    /// unless the volume is at max.
    /// If `unmute` is true, also unmute, otherwise it does not affect the mute
    /// value.
    pub async fn volume_up(&self, unmute: bool) -> Result<(), Error> {
        let procedure = if unmute {
            VcpProcedure::UnmuteRelativeVolumeUp
        } else {
            VcpProcedure::RelativeVolumeUp
        };
        self.send_control_pt(procedure).await
    }

    /// Relative volume down.  Should decrease the volume by a static step size
    /// unless the volume is at zero.
    /// If `unmute` is true, also unmute, otherwise it does not affect the mute
    /// value.
    pub async fn volume_down(&self, unmute: bool) -> Result<(), Error> {
        let procedure = if unmute {
            VcpProcedure::UnmuteRelativeVolumeDown
        } else {
            VcpProcedure::RelativeVolumeDown
        };
        self.send_control_pt(procedure).await
    }

    pub async fn mute(&self) -> Result<(), Error> {
        self.send_control_pt(VcpProcedure::Mute).await
    }

    pub async fn unmute(&self) -> Result<(), Error> {
        self.send_control_pt(VcpProcedure::Unmute).await
    }

    pub async fn set_absolute_volume(&self, setting: u8) -> Result<(), Error> {
        let change_counter = self.change_counter();
        let op = VolumeControlPointOperation {
            procedure: VcpProcedure::SetAbsoluteVolume { setting },
            change_counter,
        };
        let mut buf = [0; 3];
        op.encode(&mut buf)?;
        self.service
            .write_characteristic(&self.control_point.handle, WriteMode::None, 0, &buf)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_gatt::test_utils::*;
    use futures::Future;

    #[test]
    fn volume_state_decode() {
        let handle = bt_gatt::types::Handle(1);
        // not long enogugh
        assert!(VolumeState::from_value(handle, &[]).is_err());
        // too long is fine, but mute value is wrong.
        assert!(VolumeState::from_value(handle, &[1, 2, 3, 4]).is_err());
        // muted
        let state = VolumeState::from_value(handle, &[1, 1, 3, 4]).expect("okay");
        assert_eq!(state.mute, true);
        assert_eq!(state.setting, 1);
        assert_eq!(state.change_counter, 3);
        // not muted
        let state = VolumeState::from_value(handle, &[3, 0, 1]).expect("okay");
        assert_eq!(state.mute, false);
        assert_eq!(state.setting, 3);
        assert_eq!(state.change_counter, 1);
    }

    #[test]
    fn volume_flags_decode() {
        use bt_gatt::types::*;
        let chr = Characteristic {
            handle: Handle(1),
            uuid: VolumeFlags::UUID,
            properties: CharacteristicProperty::Read.into(),
            permissions: AttributePermissions {
                read: Some(SecurityLevels::default()),
                ..Default::default()
            },
            descriptors: Vec::new(),
        };
        // not long enogugh
        assert!(VolumeFlags::from_chr(chr.clone(), &[]).is_err());
        // persisted
        let flags = VolumeFlags::from_chr(chr.clone(), &[1]).expect("okay");
        assert_eq!(flags.persisted, VolumeSettingPersisted::UserSet);
        // not persisted (other bits ignored)
        let flags = VolumeFlags::from_chr(chr, &[2]).expect("okay");
        assert_eq!(flags.persisted, VolumeSettingPersisted::Reset);
    }

    #[track_caller]
    fn is_ready<T>(fut: impl Future<Output = T>) -> T {
        use futures::FutureExt;
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        let mut fut_pinned = std::pin::pin!(fut);
        let futures::task::Poll::Ready(x) = fut_pinned.poll_unpin(&mut noop_cx) else {
            panic!("Future is not ready");
        };
        x
    }

    fn try_from_service(
        service: FakePeerService,
    ) -> core::result::Result<VolumeControlClient<FakeTypes>, crate::Error> {
        is_ready(VolumeControlClient::<FakeTypes>::from_service(service))
    }

    const STATE_HANDLE: bt_gatt::types::Handle = bt_gatt::types::Handle(1);
    const FLAGS_HANDLE: bt_gatt::types::Handle = bt_gatt::types::Handle(2);
    const CP_HANDLE: bt_gatt::types::Handle = bt_gatt::types::Handle(3);

    fn state_chr() -> bt_gatt::types::Characteristic {
        use bt_gatt::types::*;
        Characteristic {
            handle: STATE_HANDLE,
            uuid: VolumeState::UUID,
            properties: CharacteristicProperty::Read | CharacteristicProperty::Notify,
            permissions: AttributePermissions {
                read: Some(SecurityLevels::default()),
                update: Some(SecurityLevels::default()),
                ..Default::default()
            },
            descriptors: Vec::new(),
        }
    }

    fn build_fake_service() -> FakePeerService {
        use bt_gatt::types::*;

        let mut service = FakePeerService::new();

        // No services, error (check between adding each one, should be an error until
        // all chars are added.
        assert!(try_from_service(service.clone()).is_err());
        service.add_characteristic(state_chr(), vec![1, 0, 1]);
        assert!(try_from_service(service.clone()).is_err());
        service.add_characteristic(
            Characteristic {
                handle: FLAGS_HANDLE,
                uuid: VolumeFlags::UUID,
                properties: CharacteristicProperty::Read.into(),
                permissions: AttributePermissions {
                    read: Some(SecurityLevels::default()),
                    ..Default::default()
                },
                descriptors: Vec::new(),
            },
            vec![1],
        );
        assert!(try_from_service(service.clone()).is_err());
        service.add_characteristic(
            Characteristic {
                handle: CP_HANDLE,
                uuid: VolumeControlPoint::UUID,
                properties: CharacteristicProperty::Write.into(),
                permissions: AttributePermissions {
                    write: Some(SecurityLevels::default()),
                    ..Default::default()
                },
                descriptors: Vec::new(),
            },
            vec![],
        );

        service
    }

    #[test]
    fn build_from_service() {
        use futures::{task::Poll, FutureExt};

        let mut service = build_fake_service();
        let client = try_from_service(service.clone()).unwrap();
        assert_eq!(client.change_counter(), 1);

        service.add_characteristic(state_chr(), vec![100, 1, 3]);
        let mut update_fut = Box::pin(client.update());
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        match update_fut.poll_unpin(&mut noop_cx) {
            Poll::Ready(Ok(volume)) => assert_eq!(volume, 100),
            x => panic!("Didn't update right: {x:?}"),
        }
    }

    #[test]
    fn connect() {
        let mut service = build_fake_service();
        let mut client = FakeClient::new();

        // Successfully didn't find the service
        assert!(is_ready(VolumeControlClient::<FakeTypes>::connect(&client)).unwrap().is_none());

        client.add_service(VCS_UUID, true, service.clone());

        let client = is_ready(VolumeControlClient::<FakeTypes>::connect(&client)).unwrap().unwrap();
        assert_eq!(client.change_counter(), 1);
        service.add_characteristic(state_chr(), vec![250, 1, 3]);
        assert_eq!(250, is_ready(client.update()).unwrap());
    }

    #[test]
    fn volume() {
        let mut service = build_fake_service();
        let client = try_from_service(service.clone()).unwrap();
        assert_eq!(client.change_counter(), 1);

        service.expect_characteristic_value(&CP_HANDLE, vec![0x01, client.change_counter()]);
        assert!(is_ready(client.volume_up(false)).is_ok());

        service.expect_characteristic_value(&CP_HANDLE, vec![0x03, client.change_counter()]);
        assert!(is_ready(client.volume_up(true)).is_ok());

        service.expect_characteristic_value(&CP_HANDLE, vec![0x02, client.change_counter()]);
        assert!(is_ready(client.volume_down(true)).is_ok());

        service.add_characteristic(state_chr(), vec![250, 1, 3]);
        assert_eq!(250, is_ready(client.update()).unwrap());

        service.expect_characteristic_value(&CP_HANDLE, vec![0x00, 3]);
        assert!(is_ready(client.volume_down(false)).is_ok());
    }

    #[test]
    fn mutes() {
        let mut service = build_fake_service();
        let client = try_from_service(service.clone()).unwrap();
        assert_eq!(client.change_counter(), 1);

        service.expect_characteristic_value(&CP_HANDLE, vec![0x06, 1]);
        assert!(is_ready(client.mute()).is_ok());

        service.expect_characteristic_value(&CP_HANDLE, vec![0x05, 1]);
        assert!(is_ready(client.unmute()).is_ok());

        service.expect_characteristic_value(&CP_HANDLE, vec![0x05, 1]);
        assert!(is_ready(client.unmute()).is_ok());
    }

    #[test]
    fn absolute_volume() {
        let mut service = build_fake_service();
        let client = try_from_service(service.clone()).unwrap();
        assert_eq!(client.change_counter(), 1);

        service.expect_characteristic_value(&CP_HANDLE, vec![0x04, 1, 123]);
        assert!(is_ready(client.set_absolute_volume(123)).is_ok());
    }
}
