// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A Uuid as defined by the Core Specification (v5.4, Vol 3, Part B, Sec 2.5.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uuid(uuid::Uuid);

impl Uuid {
    // Non-changing parts of the Bluetooth Base UUID, for easy comparison and
    // construction.
    const BASE_UUID_B_PART: u16 = 0x0000;
    const BASE_UUID_C_PART: u16 = 0x1000;
    const BASE_UUID_D_PART: [u8; 8] = [0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB];

    pub const fn from_u16(value: u16) -> Self {
        Self::from_u32(value as u32)
    }

    pub const fn from_u32(value: u32) -> Self {
        Uuid(uuid::Uuid::from_fields(
            value,
            Self::BASE_UUID_B_PART,
            Self::BASE_UUID_C_PART,
            &Self::BASE_UUID_D_PART,
        ))
    }

    pub fn to_u16(&self) -> Option<u16> {
        let x: u32 = self.to_u32()?;
        x.try_into().ok()
    }

    pub fn to_u32(&self) -> Option<u32> {
        let (first, second, third, final_bytes) = self.0.as_fields();
        if second != Uuid::BASE_UUID_B_PART
            || third != Uuid::BASE_UUID_C_PART
            || final_bytes != &Uuid::BASE_UUID_D_PART
        {
            return None;
        }
        Some(first)
    }

    pub fn recognize(self) -> RecognizedUuid {
        self.into()
    }
}

impl From<Uuid> for uuid::Uuid {
    fn from(value: Uuid) -> Self {
        value.0
    }
}

impl From<uuid::Uuid> for Uuid {
    fn from(value: uuid::Uuid) -> Self {
        Uuid(value)
    }
}

impl core::fmt::Display for Uuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(u) = self.to_u16() {
            return write!(f, "{u:#x}");
        }
        if let Some(u) = self.to_u32() {
            return write!(f, "{u:#x}");
        }
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl core::str::FromStr for Uuid {
    type Err = crate::packet_encoding::Error;

    fn from_str(s: &str) -> core::result::Result<Uuid, Self::Err> {
        if s.len() == 4 || s.len() == 6 {
            return match u16::from_str_radix(&s[s.len() - 4..], 16) {
                Ok(short) => Ok(Uuid::from_u16(short)),
                Err(_) => Err(crate::packet_encoding::Error::InvalidParameter(s.to_owned())),
            };
        }
        match uuid::Uuid::parse_str(s) {
            Ok(uuid) => Ok(Uuid(uuid)),
            Err(e) => Err(crate::packet_encoding::Error::Uuid(e)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssignedUuid {
    pub uuid: Uuid,
    pub name: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum RecognizedUuid {
    Assigned(AssignedUuid),
    Unrecognized(Uuid),
}

impl RecognizedUuid {
    fn as_uuid(&self) -> &Uuid {
        match self {
            RecognizedUuid::Assigned(AssignedUuid { uuid, .. }) => uuid,
            RecognizedUuid::Unrecognized(uuid) => uuid,
        }
    }
}

impl From<Uuid> for RecognizedUuid {
    fn from(value: Uuid) -> Self {
        if let Some(assigned) = characteristic_uuids::CHARACTERISTIC_UUIDS.get(&value) {
            return Self::Assigned(assigned.clone());
        }
        if let Some(assigned) = service_class::SERVICE_CLASS_UUIDS.get(&value) {
            return Self::Assigned(assigned.clone());
        }
        if let Some(assigned) = service_uuids::SERVICE_UUIDS.get(&value) {
            return Self::Assigned(assigned.clone());
        }
        Self::Unrecognized(value)
    }
}

impl std::ops::Deref for RecognizedUuid {
    type Target = Uuid;

    fn deref(&self) -> &Self::Target {
        self.as_uuid()
    }
}

impl std::ops::DerefMut for RecognizedUuid {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            RecognizedUuid::Assigned(AssignedUuid { uuid, .. }) => uuid,
            RecognizedUuid::Unrecognized(uuid) => uuid,
        }
    }
}

impl std::fmt::Display for RecognizedUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_uuid())?;
        if let RecognizedUuid::Assigned(AssignedUuid { name, .. }) = self {
            write!(f, " ({name})")?;
        }
        Ok(())
    }
}

/// Constructs an AssignedUuid from a 16-bit UUID and a name, and an optional id
/// string.
macro_rules! assigned_uuid {
    ($uuid:expr, $name:expr) => {
        AssignedUuid { uuid: Uuid::from_u16($uuid), name: String::from($name), id: None }
    };
    ($uuid:expr, $name:expr, $id:expr) => {
        AssignedUuid {
            uuid: Uuid::from_u16($uuid),
            name: String::from($name),
            id: Some(String::from($id)),
        }
    };
}

macro_rules! assigned_uuid_map {
    ( $(($uuid:expr, $name:expr, $id:expr)),* $(,)? ) => {
        {
            let mut new_map = std::collections::HashMap::new();
            $(
                let _ = new_map.insert(Uuid::from_u16($uuid), assigned_uuid!($uuid, $name, $id));
            )*
            new_map
        }
    };
    ($(($uuid:expr, $name:expr)),* $(,)? ) => {
        {
            let mut new_map = std::collections::HashMap::new();
            $(
                let _ = new_map.insert(Uuid::from_u16($uuid), assigned_uuid!($uuid, $name));
            )*
            new_map
        }
    };
}

pub mod characteristic_uuids;
pub mod descriptors;
pub mod service_class;
pub mod service_uuids;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid16() {
        let uuid = Uuid::from_u16(0x180d);
        assert_eq!(
            uuid::Uuid::parse_str("0000180d-0000-1000-8000-00805f9b34fb").unwrap(),
            uuid::Uuid::from(uuid),
        );
        // Should shorten the UUID16s
        assert_eq!("0x180d", uuid.to_string());
        let as16: u16 = uuid.to_u16().unwrap();
        assert_eq!(0x180d, as16);
    }

    #[test]
    fn uuid32() {
        let uuid = Uuid::from_u32(0xC0DECAFE);
        assert_eq!(
            uuid::Uuid::parse_str("c0decafe-0000-1000-8000-00805f9b34fb").unwrap(),
            uuid::Uuid::from(uuid),
        );
        // Should shorten the UUID16s
        assert_eq!("0xc0decafe", uuid.to_string());
        assert!(uuid.to_u16().is_none());
        let as32: u32 = uuid.to_u32().unwrap();
        assert_eq!(0xc0decafe, as32);
    }

    #[test]
    fn recognition() {
        // This is the "Running Speed and Cadence" service.
        let uuid = Uuid::from_u16(0x1814);
        assert!(uuid.recognize().to_string().contains("Cadence"));
        assert!(
            uuid.recognize().to_string().contains("0x1814"),
            "{} should contain the shortened uuid",
            uuid.to_string()
        );
        // The "Wind Chill" characteristic.
        let uuid = Uuid::from_u16(0x2A79).recognize();
        assert!(uuid.to_string().contains("Wind Chill"));
        assert!(
            uuid.to_string().contains("0x2a79"),
            "{} should contain the shortened uuid",
            uuid.to_string()
        );
        // The "VideoSource" service class uuid
        let uuid = Uuid::from_u16(0x1303).recognize();
        assert!(uuid.to_string().contains("VideoSource"));
        assert!(
            uuid.to_string().contains("0x1303"),
            "{} should contain the shortened uuid",
            uuid.to_string()
        );
    }

    #[test]
    fn assigned_uuid_map() {
        // Assigned UUID info does not need to be unique
        let test_map =
            assigned_uuid_map!((0x1234, "Test", "org.example"), (0x5678, "Test", "org.example"),);
        assert!(test_map.contains_key(&Uuid::from_u16(0x1234)));
        assert!(test_map.contains_key(&Uuid::from_u16(0x5678)));
        assert!(!test_map.contains_key(&Uuid::from_u16(0x9ABC)));

        // Assigning the same UUID twice favors the second one.
        let test_map =
            assigned_uuid_map!((0x1234, "Test", "org.example"), (0x1234, "Test 2", "com.example"),);

        assert_eq!(test_map.get(&Uuid::from_u16(0x1234)).unwrap().name, "Test 2");
    }

    #[test]
    fn parse() {
        assert_eq!("1814".parse(), Ok(Uuid::from_u16(0x1814)));
        assert_eq!("0x2a79".parse(), Ok(Uuid::from_u16(0x2a79)));
        let unknown_long_uuid = "2686f39c-bada-4658-854a-a62e7e5e8b8d";
        assert_eq!(
            unknown_long_uuid.parse(),
            Ok::<Uuid, crate::packet_encoding::Error>(
                uuid::Uuid::parse_str(unknown_long_uuid).unwrap().into()
            )
        );
    }
}
