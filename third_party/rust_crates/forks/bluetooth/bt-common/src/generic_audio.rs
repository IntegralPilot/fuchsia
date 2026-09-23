// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod codec_capabilities;
pub mod codec_configuration;
pub mod metadata_ltv;

use crate::{codable_as_bitmask, decodable_enum};

// Source:
// https://bitbucket.org/bluetooth-SIG/public/src/main/assigned_numbers/profiles_and_services/generic_audio/context_type.yaml
decodable_enum! {
    pub enum ContextType<u16, crate::packet_encoding::Error, OutOfRange> {
        Unspecified = 0x0001,
        Conversational = 0x0002,
        Media = 0x0004,
        Game = 0x0008,
        Instructional = 0x0010,
        VoiceAssistants = 0x0020,
        Live = 0x0040,
        SoundEffects = 0x0080,
        Notifications = 0x0100,
        Ringtone = 0x0200,
        Alerts = 0x0400,
        EmergencyAlarm = 0x0800,
    }
}

codable_as_bitmask!(ContextType, u16);

// Source:
// https://bitbucket.org/bluetooth-SIG/public/src/main/assigned_numbers/profiles_and_services/generic_audio/audio_location_definitions.yaml
// Regexp magic for quick variants:
// %s/ - value: \(\S\+\)\n   audio_location: \(.*\)\n/\2 = \1\r,/g
// with subsequent removal of Spaces
decodable_enum! {
    pub enum AudioLocation<u32, crate::packet_encoding::Error, OutOfRange> {
        FrontLeft = 0x00000001,
        FrontRight = 0x00000002,
        FrontCenter = 0x00000004,
        LowFrequencyEffects1 = 0x00000008,
        BackLeft = 0x00000010,
        BackRight = 0x00000020,
        FrontLeftOfCenter = 0x00000040,
        FrontRightOfCenter = 0x00000080,
        BackCenter = 0x00000100,
        LowFrequencyEffects2 = 0x00000200,
        SideLeft = 0x00000400,
        SideRight = 0x00000800,
        TopFrontLeft = 0x00001000,
        TopFrontRight = 0x00002000,
        TopFrontCenter = 0x00004000,
        TopCenter = 0x00008000,
        TopBackLeft = 0x00010000,
        TopBackRight = 0x00020000,
        TopSideLeft = 0x00040000,
        TopSideRight = 0x00080000,
        TopBackCenter = 0x00100000,
        BottomFrontCenter = 0x00200000,
        BottomFrontLeft = 0x00400000,
        BottomFrontRight = 0x00800000,
        FrontLeftWide = 0x01000000,
        FrontRightWide = 0x02000000,
        LeftSurround = 0x04000000,
        RightSurround = 0x08000000,
    }
}

codable_as_bitmask!(AudioLocation, u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_decodable() {
        let five_point_one = 0b111111;

        let locations: std::collections::HashSet<AudioLocation> =
            AudioLocation::from_bits(five_point_one).collect();

        assert_eq!(6, locations.len());

        let expected_locations = [
            AudioLocation::FrontLeft,
            AudioLocation::FrontRight,
            AudioLocation::FrontCenter,
            AudioLocation::LowFrequencyEffects1,
            AudioLocation::BackLeft,
            AudioLocation::BackRight,
        ]
        .into_iter()
        .collect();

        assert_eq!(locations, expected_locations);

        assert_eq!(AudioLocation::try_from(0x4), Ok(AudioLocation::FrontCenter));

        // Directly decoding a location that is not a single bit is an error.
        assert!(AudioLocation::try_from(0b1010101).is_err());
    }

    #[test]
    fn locations_encodable() {
        let locations_missing_sub = [
            AudioLocation::FrontLeft,
            AudioLocation::FrontRight,
            AudioLocation::FrontCenter,
            AudioLocation::BackLeft,
            AudioLocation::BackRight,
        ];

        let value = AudioLocation::to_bits(locations_missing_sub.iter());

        assert_eq!(0b110111, value);
    }

    #[test]
    fn context_type_decodable() {
        let contexts: Vec<ContextType> = ContextType::from_bits(0b10).collect();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0], ContextType::Conversational);

        let live_and_instructional = 0b1010000;
        let contexts: std::collections::HashSet<ContextType> =
            ContextType::from_bits(live_and_instructional).collect();

        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts, [ContextType::Live, ContextType::Instructional].into_iter().collect());

        let alerts_and_conversational = 0x0402;

        let contexts: std::collections::HashSet<ContextType> =
            ContextType::from_bits(alerts_and_conversational).collect();

        assert_eq!(contexts.len(), 2);
        assert_eq!(
            contexts,
            [ContextType::Alerts, ContextType::Conversational].into_iter().collect()
        );
    }

    #[test]
    fn context_type_encodable() {
        let contexts = [ContextType::Notifications, ContextType::SoundEffects, ContextType::Game];

        let value = ContextType::to_bits(contexts.iter());

        assert_eq!(0x188, value);
    }
}
