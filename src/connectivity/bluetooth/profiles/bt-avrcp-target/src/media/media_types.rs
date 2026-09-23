// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_avrcp::{self as fidl_avrcp, MediaAttributes, PlayStatus};
use fidl_fuchsia_media::{self as fidl_media_types, Metadata, TimelineFunction};
use fidl_table_validation::ValidFidlTable;
use log::{debug, trace, warn};
use {fidl_fuchsia_media_sessions2 as fidl_media, fuchsia_async as fasync};

/// Converts time (i64, in nanoseconds) to milliseconds (u32).
fn time_nanos_to_millis(t: i64) -> u32 {
    zx::MonotonicDuration::from_nanos(t).into_millis() as u32
}

/// Returns the current position of playing media in milliseconds.
/// Using formula defined in: fuchsia.media.TimelineFunction.
/// s = (r - reference_time) * (subject_delta / reference_delta) + subject_time
/// where `s` = song_position, `r` = current time.
/// If the `reference_delta` in `t` is 0, this violates the `fuchsia.media.TimelineFunction`
/// contract.
fn media_timeline_fn_to_position(t: TimelineFunction, current_time: i64) -> Option<u32> {
    let diff = current_time - t.reference_time;
    let ratio = if t.reference_delta == 0 {
        warn!("Reference delta is zero. Violation of TimelineFunction API.");
        return None;
    } else {
        (t.subject_delta / t.reference_delta) as i64
    };

    let position_nanos = diff * ratio + t.subject_time;

    // Some media sources report the reference time ahead of the current time. This is OK.
    // However, since AVRCP does not report negative playback positions, clamp the position at 0.
    if position_nanos.is_negative() {
        return Some(0);
    }

    Some(time_nanos_to_millis(position_nanos))
}

pub fn media_repeat_mode_to_avrcp(src: fidl_media::RepeatMode) -> fidl_avrcp::RepeatStatusMode {
    match src {
        fidl_media::RepeatMode::Off => fidl_avrcp::RepeatStatusMode::Off,
        fidl_media::RepeatMode::Group => fidl_avrcp::RepeatStatusMode::GroupRepeat,
        fidl_media::RepeatMode::Single => fidl_avrcp::RepeatStatusMode::SingleTrackRepeat,
    }
}

pub fn avrcp_repeat_mode_to_media(mode: fidl_avrcp::RepeatStatusMode) -> fidl_media::RepeatMode {
    match mode {
        fidl_avrcp::RepeatStatusMode::SingleTrackRepeat => fidl_media::RepeatMode::Single,
        fidl_avrcp::RepeatStatusMode::GroupRepeat
        | fidl_avrcp::RepeatStatusMode::AllTrackRepeat => fidl_media::RepeatMode::Group,
        _ => fidl_media::RepeatMode::Off,
    }
}

pub fn media_shuffle_mode_to_avrcp(shuffle_enabled: bool) -> fidl_avrcp::ShuffleMode {
    if shuffle_enabled {
        fidl_avrcp::ShuffleMode::AllTrackShuffle
    } else {
        fidl_avrcp::ShuffleMode::Off
    }
}

pub fn avrcp_shuffle_mode_to_media(src: fidl_avrcp::ShuffleMode) -> bool {
    match src {
        fidl_avrcp::ShuffleMode::AllTrackShuffle | fidl_avrcp::ShuffleMode::GroupShuffle => true,
        _ => false,
    }
}

pub fn media_player_state_to_playback_status(
    src: fidl_media::PlayerState,
) -> fidl_avrcp::PlaybackStatus {
    match src {
        fidl_media::PlayerState::Idle => fidl_avrcp::PlaybackStatus::Stopped,
        fidl_media::PlayerState::Playing => fidl_avrcp::PlaybackStatus::Playing,
        fidl_media::PlayerState::Paused | fidl_media::PlayerState::Buffering => {
            fidl_avrcp::PlaybackStatus::Paused
        }
        fidl_media::PlayerState::Error => fidl_avrcp::PlaybackStatus::Error,
    }
}

#[derive(Clone, Debug, Default, PartialEq, ValidFidlTable)]
#[fidl_table_src(PlayStatus)]
pub(crate) struct ValidPlayStatus {
    #[fidl_field_type(optional)]
    /// The length of media in millis.
    song_length: Option<u32>,
    #[fidl_field_type(optional)]
    /// The current position of media in millis.
    pub song_position: Option<u32>,
    #[fidl_field_type(optional)]
    /// The current playback status of media.
    playback_status: Option<fidl_avrcp::PlaybackStatus>,
}

impl ValidPlayStatus {
    #[cfg(test)]
    pub(crate) fn new(
        song_length: Option<u32>,
        song_position: Option<u32>,
        playback_status: Option<fidl_avrcp::PlaybackStatus>,
    ) -> Self {
        Self { song_length, song_position, playback_status }
    }

    /// Return the current playback status of currently playing media.
    /// If there is no currently playing media (i.e `playback_status` is None),
    /// return PlaybackStatus::Stopped, as this is mandatory.
    /// See AVRCP 1.6.2 Section 6.7.1.
    pub fn get_playback_status(&self) -> fidl_avrcp::PlaybackStatus {
        self.playback_status.map_or(fidl_avrcp::PlaybackStatus::Stopped, |s| s)
    }

    /// Update the play status from updates from Media.
    /// If the `duration` is None, then the value is considered unknown, unavailable, or unchanged.
    /// If the `rate` is None, then the playback rate is considered unchanged.
    /// If the `player_state` is None, then the media player state is considered unchanged.
    /// `duration` is received in nanoseconds and is saved as milliseconds.
    pub(crate) fn update_play_status(
        &mut self,
        duration: Option<i64>,
        rate: Option<PlaybackRate>,
        player_state: Option<fidl_media::PlayerState>,
    ) {
        // The `duration` is always the most up-to-date value so it is always used.
        self.song_length = duration.map(|d| time_nanos_to_millis(d));
        if let Some(rate) = rate {
            self.song_position = rate.current_position();
        }
        if let Some(state) = player_state {
            self.playback_status = Some(media_player_state_to_playback_status(state));
        }
    }
}

/// The time, in nanos, that a notification must be resolved by.
pub type NotificationTimeout = zx::MonotonicDuration;

/// The current playback rate of media.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackRate(TimelineFunction);

impl PlaybackRate {
    /// Returns the playback rate.
    fn rate(&self) -> f64 {
        if self.0.reference_delta == 0 {
            // No reasonable rate here, return stopped.
            return 0.0;
        }
        self.0.subject_delta as f64 / self.0.reference_delta as f64
    }

    /// Returns the current subject time based on the TimelineFunction, in milliseconds.
    /// Returns Some(0) if the current position is negative.
    /// Returns None if the current position cannot be calculated.
    pub fn current_position(&self) -> Option<u32> {
        media_timeline_fn_to_position(self.0, fasync::MonotonicInstant::now().into_nanos())
    }

    /// Given a duration in playback time, returns the equal duration in reference time
    /// (usually monotonic clock duration).
    ///
    /// For example, if the current playback rate = 3/2 (i.e fast-forward),
    /// and the `change_nanos` is 5e9 nanos (i.e 5 seconds), the scaled deadline is
    /// current_time + pos_change_interval * (1 / playback_rate).
    ///
    /// If the current playback rate is stopped (i.e 0), `None` is returned as
    /// there is no response deadline.
    pub(crate) fn reference_deadline(
        &self,
        change: zx::MonotonicDuration,
    ) -> Option<NotificationTimeout> {
        let rate = self.rate();
        if rate == 0.0 {
            return None;
        }

        let timeout = ((change.into_nanos() as f64) * (1.0 / rate)) as i64;
        Some(zx::MonotonicDuration::from_nanos(timeout))
    }
}

/// Represents a timeline function for media that is currently stopped. Defined in
/// https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.media/timeline_function.fidl;l=41?
fn stopped_timeline_function() -> TimelineFunction {
    TimelineFunction { subject_time: 0, reference_time: 0, subject_delta: 0, reference_delta: 1 }
}

impl Default for PlaybackRate {
    /// The default playback rate is stopped (i.e 0).
    fn default() -> Self {
        Self(stopped_timeline_function())
    }
}

impl From<TimelineFunction> for PlaybackRate {
    fn from(timeline_fn: TimelineFunction) -> Self {
        Self(timeline_fn)
    }
}

/// The PlayerApplicationSettings for the MediaSession.
/// Currently, only `repeat_status_mode` and `shuffle_mode` are supported by
/// Players in Media.
/// `equalizer` and `scan_mode` are present for correctness in mapping to AVRCP,
/// but are not used or set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValidPlayerApplicationSettings {
    equalizer: Option<fidl_avrcp::Equalizer>,
    repeat_status_mode: Option<fidl_avrcp::RepeatStatusMode>,
    shuffle_mode: Option<fidl_avrcp::ShuffleMode>,
    scan_mode: Option<fidl_avrcp::ScanMode>,
    // TODO(https://fxbug.dev/42117316): Add support to handle custom attributes.
}

impl ValidPlayerApplicationSettings {
    pub fn new(
        equalizer: Option<fidl_avrcp::Equalizer>,
        repeat_status_mode: Option<fidl_avrcp::RepeatStatusMode>,
        shuffle_mode: Option<fidl_avrcp::ShuffleMode>,
        scan_mode: Option<fidl_avrcp::ScanMode>,
    ) -> Self {
        Self { equalizer, repeat_status_mode, shuffle_mode, scan_mode }
    }

    pub fn repeat_status_mode(&self) -> Option<fidl_avrcp::RepeatStatusMode> {
        self.repeat_status_mode
    }

    pub fn shuffle_mode(&self) -> Option<fidl_avrcp::ShuffleMode> {
        self.shuffle_mode
    }

    /// Return the current RepeatStatusMode.
    /// If it is not set, default to OFF.
    pub fn get_repeat_status_mode(&self) -> fidl_avrcp::RepeatStatusMode {
        self.repeat_status_mode.map_or(fidl_avrcp::RepeatStatusMode::Off, |s| s)
    }

    /// Return the current ShuffleMode.
    /// If it is not set, default to OFF.
    pub fn get_shuffle_mode(&self) -> fidl_avrcp::ShuffleMode {
        self.shuffle_mode.map_or(fidl_avrcp::ShuffleMode::Off, |s| s)
    }

    /// Given an attribute specified by `attribute_id`, sets the field to None.
    pub fn clear_attribute(
        &mut self,
        attribute_id: fidl_avrcp::PlayerApplicationSettingAttributeId,
    ) {
        use fidl_avrcp::PlayerApplicationSettingAttributeId as avrcp_id;
        match attribute_id {
            avrcp_id::Equalizer => self.equalizer = None,
            avrcp_id::ScanMode => self.scan_mode = None,
            avrcp_id::RepeatStatusMode => self.repeat_status_mode = None,
            avrcp_id::ShuffleMode => self.shuffle_mode = None,
        };
    }

    /// Returns true if the player application settings contains unsupported settings.
    pub fn has_unsupported_settings(&self) -> bool {
        self.equalizer.is_some() || self.scan_mode.is_some()
    }

    /// Update the settings, if the settings are present. Otherwise, ignore the update.
    pub fn update_player_application_settings(
        &mut self,
        repeat_mode: Option<fidl_media::RepeatMode>,
        shuffle_on: Option<bool>,
    ) {
        if let Some(repeat) = repeat_mode {
            self.update_repeat_status_mode(repeat);
        }
        if let Some(shuffle) = shuffle_on {
            self.update_shuffle_mode(shuffle);
        }
    }

    /// Update the `repeat_status_mode` from a change in MediaPlayer.
    pub fn update_repeat_status_mode(&mut self, repeat_mode: fidl_media::RepeatMode) {
        self.repeat_status_mode = Some(media_repeat_mode_to_avrcp(repeat_mode));
    }

    /// Update the `shuffle_mode` from a change in MediaPlayer.
    pub fn update_shuffle_mode(&mut self, shuffle_mode: bool) {
        self.shuffle_mode = Some(media_shuffle_mode_to_avrcp(shuffle_mode));
    }

    /// Sets the `repeat_status_mode`.
    pub fn set_repeat_status_mode(&mut self, status: Option<fidl_avrcp::RepeatStatusMode>) {
        self.repeat_status_mode = status;
    }

    /// Sets the `shuffle_mode`.
    pub fn set_shuffle_mode(&mut self, status: Option<fidl_avrcp::ShuffleMode>) {
        self.shuffle_mode = status;
    }
}

impl From<fidl_avrcp::PlayerApplicationSettings> for ValidPlayerApplicationSettings {
    fn from(src: fidl_avrcp::PlayerApplicationSettings) -> ValidPlayerApplicationSettings {
        ValidPlayerApplicationSettings::new(
            src.equalizer.map(Into::into),
            src.repeat_status_mode.map(Into::into),
            src.shuffle_mode.map(Into::into),
            src.scan_mode.map(Into::into),
        )
    }
}

impl From<ValidPlayerApplicationSettings> for fidl_avrcp::PlayerApplicationSettings {
    fn from(src: ValidPlayerApplicationSettings) -> fidl_avrcp::PlayerApplicationSettings {
        fidl_avrcp::PlayerApplicationSettings {
            equalizer: src.equalizer.map(Into::into),
            repeat_status_mode: src.repeat_status_mode.map(Into::into),
            shuffle_mode: src.shuffle_mode.map(Into::into),
            scan_mode: src.scan_mode.map(Into::into),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, ValidFidlTable)]
#[fidl_table_src(MediaAttributes)]
pub struct MediaInfo {
    #[fidl_field_type(optional)]
    title: Option<String>,
    #[fidl_field_type(optional)]
    artist_name: Option<String>,
    #[fidl_field_type(optional)]
    album_name: Option<String>,
    #[fidl_field_type(optional)]
    track_number: Option<String>,
    #[fidl_field_type(optional)]
    genre: Option<String>,
    #[fidl_field_type(optional)]
    total_number_of_tracks: Option<String>,
    #[fidl_field_type(optional)]
    playing_time: Option<String>,
}

impl MediaInfo {
    pub fn new(
        title: Option<String>,
        artist_name: Option<String>,
        album_name: Option<String>,
        track_number: Option<String>,
        genre: Option<String>,
        total_number_of_tracks: Option<String>,
        playing_time: Option<String>,
    ) -> Self {
        Self {
            title,
            artist_name,
            album_name,
            track_number,
            genre,
            total_number_of_tracks,
            playing_time,
        }
    }

    /// Returns 0x0 if there is metadata present, implying there is currently set media.
    /// Otherwise returns u64 MAX.
    /// The track ID value is defined in AVRCP 1.6.2 Section 6.7.2, Table 6.32.
    pub fn get_track_id(&self) -> u64 {
        if self.title.is_some()
            || self.artist_name.is_some()
            || self.album_name.is_some()
            || self.track_number.is_some()
            || self.genre.is_some()
        {
            0
        } else {
            u64::MAX
        }
    }

    pub fn track_changed(&self, other: &Self) -> bool {
        self.title != other.title
            || self.artist_name != other.artist_name
            || self.album_name != other.album_name
            || self.track_number != other.track_number
    }

    /// Updates information about currently playing media.
    /// If a metadata update is present, this implies the current media has changed.
    /// In the event that metadata is present, but duration is not, `playing_time`
    /// will default to None. See `update_playing_time()`.
    ///
    /// If no metadata is present, but a duration is present, update only `playing_time`.
    /// The rationale here is a use-case such as a live stream, where the playing time
    /// may be constantly changing, but stream metadata remains constant.
    pub fn update_media_info(
        &mut self,
        media_duration: Option<i64>,
        media_metadata: Option<Metadata>,
    ) {
        if let Some(metadata) = media_metadata {
            self.update_metadata(metadata);
        }
        self.update_playing_time(media_duration);
    }

    /// Take the duration of media, in nanos, and store as duration in millis as a string.
    /// If no duration is present, `playing_time` will be set to None.
    fn update_playing_time(&mut self, duration: Option<i64>) {
        self.playing_time = duration.map(|d| time_nanos_to_millis(d).to_string());
    }

    pub fn update_metadata(&mut self, metadata: Metadata) {
        for property in metadata.properties {
            match property.label.as_str() {
                fidl_media_types::METADATA_LABEL_TITLE => {
                    self.title = Some(property.value);
                }
                fidl_media_types::METADATA_LABEL_ARTIST => {
                    self.artist_name = Some(property.value);
                }
                fidl_media_types::METADATA_LABEL_ALBUM => {
                    self.album_name = Some(property.value);
                }
                fidl_media_types::METADATA_LABEL_TRACK_NUMBER => {
                    self.track_number = Some(property.value);
                }
                fidl_media_types::METADATA_LABEL_GENRE => {
                    self.genre = Some(property.value);
                }
                _ => {
                    trace!(
                        value:% = property.value; "Media metadata ({:?}) not supported",
                        property.label,
                    );
                }
            }
        }
        debug!("Media metadata after update: {:?}", self);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Notification {
    pub status: Option<fidl_avrcp::PlaybackStatus>,
    // Media info currently used to detect track changes when track_id is 0
    pub media_info: Option<MediaInfo>,
    pub pos: Option<u32>,
    pub battery_status: Option<fidl_avrcp::BatteryStatus>,
    // SystemStatus
    pub application_settings: Option<ValidPlayerApplicationSettings>,
    pub player_id: Option<u16>,
    pub volume: Option<u8>,
    pub device_connected: Option<bool>,
}

impl Notification {
    fn new(
        status: Option<fidl_avrcp::PlaybackStatus>,
        pos: Option<u32>,
        battery_status: Option<fidl_avrcp::BatteryStatus>,
        application_settings: Option<ValidPlayerApplicationSettings>,
        player_id: Option<u16>,
        volume: Option<u8>,
        device_connected: Option<bool>,
    ) -> Self {
        Self {
            status,
            pos,
            battery_status,
            application_settings,
            player_id,
            volume,
            device_connected,
            media_info: None,
        }
    }

    /// Returns a `Notification` with only the field specified by `event_id` set.
    /// If `event_id` is not supported, the default empty `Notification` will be returned.
    pub fn only_event(&self, event_id: &fidl_avrcp::NotificationEvent) -> Self {
        let mut res = Notification::default();
        match event_id {
            fidl_avrcp::NotificationEvent::PlaybackStatusChanged => {
                res.status = self.status;
            }
            fidl_avrcp::NotificationEvent::TrackChanged => {
                res.media_info = self.media_info.clone();
            }
            fidl_avrcp::NotificationEvent::TrackPosChanged => {
                res.pos = self.pos;
            }
            fidl_avrcp::NotificationEvent::BattStatusChanged => {
                res.battery_status = self.battery_status;
            }
            fidl_avrcp::NotificationEvent::PlayerApplicationSettingChanged => {
                res.application_settings = self.application_settings.clone();
            }
            fidl_avrcp::NotificationEvent::AddressedPlayerChanged => {
                res.player_id = self.player_id;
            }
            fidl_avrcp::NotificationEvent::VolumeChanged => {
                res.volume = self.volume;
            }
            _ => warn!("Event id {:?} is not supported for Notification.", event_id),
        }
        res
    }
}

impl From<fidl_avrcp::Notification> for Notification {
    fn from(src: fidl_avrcp::Notification) -> Notification {
        Notification::new(
            src.status.map(Into::into),
            src.pos,
            src.battery_status,
            src.application_settings.map(|s| s.try_into().expect("Couldn't convert PAS")),
            src.player_id,
            src.volume,
            src.device_connected,
        )
    }
}

impl From<Notification> for fidl_avrcp::Notification {
    fn from(src: Notification) -> fidl_avrcp::Notification {
        let mut res = fidl_avrcp::Notification::default();

        res.status = src.status.map(Into::into);
        res.track_id = src.media_info.as_ref().map(MediaInfo::get_track_id);
        res.pos = src.pos;
        res.battery_status = src.battery_status;
        res.application_settings = src.application_settings.map(Into::into);
        res.player_id = src.player_id;
        res.volume = src.volume;
        res.device_connected = src.device_connected;
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    /// Tests correctness of updating and getting the playback rate from the PlaybackRate.
    fn test_playback_rate() {
        let mut pbr = PlaybackRate::default();
        assert_eq!(0.0, pbr.rate());

        let timeline_fn = TimelineFunction {
            subject_time: 0,
            reference_time: 0,
            subject_delta: 10,
            reference_delta: 5,
        };
        pbr = timeline_fn.into();
        assert_eq!(2.0, pbr.rate());

        let timeline_fn = TimelineFunction {
            subject_time: 0,
            reference_time: 0,
            subject_delta: 4,
            reference_delta: 10,
        };
        pbr = timeline_fn.into();
        assert_eq!(0.4, pbr.rate());
    }

    #[fuchsia::test]
    /// Tests correctness of calculating the response deadline given a playback rate.
    fn test_playback_rate_reference_deadline() {
        // Note: subject and reference time can be anything here, the ratio of subject to reference
        // delta is the speed of playback.
        // Fast forward,
        let ff_rate = PlaybackRate(TimelineFunction {
            subject_time: 0,
            reference_time: 0,
            subject_delta: 10,
            reference_delta: 4,
        });
        let deadline = ff_rate.reference_deadline(zx::MonotonicDuration::from_nanos(1000000000));
        let expected = zx::MonotonicDuration::from_nanos(400000000);
        assert_eq!(Some(expected), deadline);

        // Normal playback,
        let play_rate = PlaybackRate(TimelineFunction {
            subject_time: 0,
            reference_time: 0,
            subject_delta: 1,
            reference_delta: 1,
        });
        let deadline = play_rate.reference_deadline(zx::MonotonicDuration::from_nanos(5000000000));
        let expected = zx::MonotonicDuration::from_nanos(5000000000);
        assert_eq!(Some(expected), deadline);

        // Slow motion.
        let slow_rate = PlaybackRate(TimelineFunction {
            subject_time: 0,
            reference_time: 0,
            subject_delta: 3,
            reference_delta: 4,
        });
        let deadline = slow_rate.reference_deadline(zx::MonotonicDuration::from_nanos(9000000));
        let expected = zx::MonotonicDuration::from_nanos(12000000);
        assert_eq!(Some(expected), deadline);

        // Stopped playback - no deadline.
        let stop_rate = PlaybackRate(stopped_timeline_function());
        let deadline = stop_rate.reference_deadline(zx::MonotonicDuration::from_nanos(900000000));
        assert_eq!(None, deadline);
    }

    #[fuchsia::test]
    /// Tests correctness of updating the `playing_time` field in MediaInfo.
    fn test_media_info_update_playing_time() {
        let mut info: MediaInfo = Default::default();
        assert_eq!(info.get_track_id(), u64::MAX);

        // Duration (in nanos), roughly 12 milliseconds.
        let duration = Some(12345678);
        let expected_duration = 12;
        info.update_playing_time(duration);
        assert_eq!(Some(expected_duration.to_string()), info.playing_time);
        assert_eq!(u64::MAX, info.get_track_id());
    }

    #[fuchsia::test]
    /// Tests correctness of updating media-related metadata.
    fn test_media_info_update_metadata() {
        let mut info: MediaInfo = Default::default();

        let sample_title = "This is a sample title".to_string();
        let sample_genre = "Pop".to_string();
        let metadata = fidl_media_types::Metadata {
            properties: vec![
                fidl_media_types::Property {
                    label: fidl_media_types::METADATA_LABEL_TITLE.to_string(),
                    value: sample_title.clone(),
                },
                fidl_media_types::Property {
                    label: fidl_media_types::METADATA_LABEL_GENRE.to_string(),
                    value: sample_genre.clone(),
                },
                // Unsupported piece of metadata, should be ignored.
                fidl_media_types::Property {
                    label: fidl_media_types::METADATA_LABEL_COMPOSER.to_string(),
                    value: "Bach".to_string(),
                },
            ],
        };

        info.update_metadata(metadata);
        assert_eq!(Some(sample_title), info.title);
        assert_eq!(Some(sample_genre), info.genre);
        assert_eq!(0, info.get_track_id());
    }

    #[fuchsia::test]
    /// Tests updating media_info with `metadata` but no `duration` defaults `duration` = None.
    fn test_media_info_update_metadata_no_duration() {
        let mut info: MediaInfo = Default::default();

        let sample_title = "Foobar".to_string();
        let metadata = fidl_media_types::Metadata {
            properties: vec![fidl_media_types::Property {
                label: fidl_media_types::METADATA_LABEL_TITLE.to_string(),
                value: sample_title.clone(),
            }],
        };

        info.update_media_info(None, Some(metadata));
        assert_eq!(None, info.playing_time);
        assert_eq!(Some(sample_title), info.title);
        assert_eq!(0, info.get_track_id());
    }

    #[fuchsia::test]
    /// Tests updating media_info with no metadata updates preserves original values, but
    /// overwrites `playing_time` since duration is not a top-level SessionInfoDelta update.
    /// Tests correctness of conversion to `fidl_avrcp::MediaAttributes` type.
    fn test_media_info_update_no_metadata() {
        let mut info: MediaInfo = Default::default();
        let duration = Some(22345678);

        // Create original state (metadata and random duration).
        let sample_title = "Foobar".to_string();
        let metadata = fidl_media_types::Metadata {
            properties: vec![fidl_media_types::Property {
                label: fidl_media_types::METADATA_LABEL_TITLE.to_string(),
                value: sample_title.clone(),
            }],
        };
        info.update_media_info(None, Some(metadata));
        info.update_playing_time(duration);

        // Metadata should be preserved, except `playing_time`.
        info.update_media_info(None, None);
        assert_eq!(None, info.playing_time);
        assert_eq!(Some(sample_title.clone()), info.title);

        let info_fidl: fidl_avrcp::MediaAttributes = info.into();
        assert_eq!(Some(sample_title), info_fidl.title);
        assert_eq!(None, info_fidl.artist_name);
        assert_eq!(None, info_fidl.album_name);
        assert_eq!(None, info_fidl.track_number);
        assert_eq!(None, info_fidl.total_number_of_tracks);
        assert_eq!(None, info_fidl.playing_time);
    }

    #[fuchsia::test]
    /// Creates ValidPlayerApplicationSettings.
    /// Tests correctness of updating `repeat_status_mode` and `shuffle_mode`.
    /// Tests updating the settings with no updates preserves original values.
    /// Tests correctness of conversion to `fidl_avrcp::PlayerApplicationSettings` type.
    /// Tests correctness of clearing a field.
    fn test_player_application_settings() {
        let mut settings = ValidPlayerApplicationSettings::new(None, None, None, None);
        assert_eq!(settings.has_unsupported_settings(), false);

        let repeat_mode = Some(fidl_media::RepeatMode::Group);
        let shuffle_mode = Some(true);
        settings.update_player_application_settings(repeat_mode, shuffle_mode);

        let expected_repeat_mode = Some(fidl_avrcp::RepeatStatusMode::GroupRepeat);
        let expected_shuffle_mode = Some(fidl_avrcp::ShuffleMode::AllTrackShuffle);
        assert_eq!(expected_repeat_mode, settings.repeat_status_mode);
        assert_eq!(expected_shuffle_mode, settings.shuffle_mode);
        assert_eq!(settings.has_unsupported_settings(), false);

        settings.update_player_application_settings(None, None);
        assert_eq!(expected_repeat_mode, settings.repeat_status_mode);
        assert_eq!(expected_shuffle_mode, settings.shuffle_mode);

        let settings_fidl: fidl_avrcp::PlayerApplicationSettings = settings.clone().into();
        assert_eq!(
            Some(fidl_avrcp::RepeatStatusMode::GroupRepeat),
            settings_fidl.repeat_status_mode
        );
        assert_eq!(Some(fidl_avrcp::ShuffleMode::AllTrackShuffle), settings_fidl.shuffle_mode);
        assert_eq!(None, settings_fidl.equalizer);
        assert_eq!(None, settings_fidl.scan_mode);

        settings.clear_attribute(fidl_avrcp::PlayerApplicationSettingAttributeId::ShuffleMode);
        assert_eq!(None, settings.shuffle_mode);
    }

    #[fuchsia::test]
    /// Creates PlayStatus.
    /// Tests correctness of updating with duration, timeline_fn, and player_state from Media.
    /// Tests correctness of conversion to fidl_avrcp::PlayStatus.
    fn test_play_status() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(900000000));

        let mut play_status: ValidPlayStatus = Default::default();
        assert_eq!(play_status.get_playback_status(), fidl_avrcp::PlaybackStatus::Stopped);

        let player_state = Some(fidl_media::PlayerState::Buffering);
        // Receive an update with only a change in player state.
        play_status.update_play_status(None, None, player_state);

        assert_eq!(play_status.song_length, None);
        assert_eq!(play_status.song_position, None);
        assert_eq!(play_status.playback_status, Some(fidl_avrcp::PlaybackStatus::Paused));
        assert_eq!(play_status.get_playback_status(), fidl_avrcp::PlaybackStatus::Paused);

        let duration = Some(9876543210); // nanos
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 1000,        // nanos
            reference_time: 800000000, // nanos
            subject_delta: 1,          // Normal playback rate
            reference_delta: 1,        // Normal playback rate
        };
        let player_state = Some(fidl_media::PlayerState::Playing);
        play_status.update_play_status(duration, Some(timeline_fn.into()), player_state);

        let expected_song_length = Some(9876); // millis
        let expected_song_position = 100; // (100000000 + 1000) / 10^6
        let expected_player_state = fidl_avrcp::PlaybackStatus::Playing;
        assert_eq!(play_status.song_length, expected_song_length);
        assert_eq!(play_status.song_position, Some(expected_song_position));
        assert_eq!(play_status.playback_status, Some(expected_player_state));
        assert_eq!(play_status.get_playback_status(), expected_player_state);

        // Verifies the conversion into the AVRCP FIDL type.
        let play_status_fidl: fidl_avrcp::PlayStatus = play_status.clone().into();
        let expected_avrcp_play_status = fidl_avrcp::PlayStatus {
            song_length: expected_song_length,
            song_position: Some(expected_song_position),
            playback_status: Some(expected_player_state),
            ..Default::default()
        };
        assert_eq!(play_status_fidl, expected_avrcp_play_status);

        // An "empty" update should only affect the song length as song position and player state
        // are assumed to be unchanged.
        play_status.update_play_status(None, None, None);
        assert_eq!(play_status.song_length, None);
        assert_eq!(play_status.get_playback_status(), expected_player_state);
    }

    #[fuchsia::test]
    /// Tests the timeline function to song_position conversion.
    /// 1. Normal case with media playing.
    /// 2. Normal case with media paused.
    /// 3. Error case with invalid timeline function -> should return None.
    /// 4. Normal case with media in fast forwarding.
    /// 5. Normal case where reference time is in the future.
    ///   a. The calculated song position is positive and returned.
    ///   b. The calculated song position is negative, and clamped at 0.
    fn test_timeline_fn_to_song_position() {
        // 1. Normal case, media is playing.
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 0,           // Playback started at beginning of media.
            reference_time: 500000000, // Monotonic clock time at beginning of media (nanos).
            subject_delta: 1,          // Normal playback rate
            reference_delta: 1,        // Normal playback rate
        };
        // Current time of the system (nanos).
        let curr_time = 520060095;
        let song_position = media_timeline_fn_to_position(timeline_fn, curr_time);
        let expected_position = 20; // 20060095 / 1000000 = 520millis
        assert_eq!(song_position, Some(expected_position));

        // 2. Normal case, media is paused.
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 534912992,   // Playback started at a random time.
            reference_time: 500000000, // Monotonic clock time at beginning of media (nanos).
            subject_delta: 0,          // Paused playback rate
            reference_delta: 1,        // Paused playback rate
        };
        // Current time of the system.
        let curr_time = 500060095;
        let song_position = media_timeline_fn_to_position(timeline_fn, curr_time);
        // The expected position of media should be when it was started (subject_time), since
        // it's paused.
        let expected_position = 534; // 534973087 / 1000000 = 534 millis.
        assert_eq!(song_position, Some(expected_position));

        // 3. Invalid case, `reference_delta` = 0, which violates the MediaSession contract.
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 534912992,   // Playback started at a random time.
            reference_time: 500000000, // Monotonic clock time at beginning of media (nanos).
            subject_delta: 0,          // Paused playback rate
            reference_delta: 0,        // Invalid playback rate
        };
        // Current time of the system.
        let curr_time = 500060095;
        let song_position = media_timeline_fn_to_position(timeline_fn, curr_time);
        assert!(song_position.is_none());

        // 4. Fast-forward case, the ratio is > 1, so the media is in fast-forward mode.
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 500,         // Playback started at a random time.
            reference_time: 500000000, // Monotonic clock time at beginning of media (nanos).
            subject_delta: 2,          // Fast-forward playback rate
            reference_delta: 1,        // Fast-forward playback rate
        };
        // Current time of the system.
        let curr_time = 500760095;
        let song_position = media_timeline_fn_to_position(timeline_fn, curr_time);
        let expected_position = 1; // 1520690 / 1000000 = 1 millis
        assert_eq!(song_position, Some(expected_position));

        // 5a. Future reference time, but the calculated position is positive.
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 123456789,   // Playback started at a random time.
            reference_time: 500010000, // Monotonic clock time at beginning of media (nanos).
            subject_delta: 1,          // Normal playback rate
            reference_delta: 1,        // Normal playback rate
        };
        // Current time of the system.
        let curr_time = 500000000;
        let song_position = media_timeline_fn_to_position(timeline_fn, curr_time);
        let expected_position = 123; //  -10000 + 123456789 = 123446789 nanos = 123ms
        assert_eq!(song_position, Some(expected_position));

        // 5b. Future reference time, but the calculated position is negative.
        let timeline_fn = fidl_media_types::TimelineFunction {
            subject_time: 0,           // Playback started at a random time.
            reference_time: 500010000, // Monotonic clock time at beginning of media (nanos).
            subject_delta: 1,          // Normal playback rate
            reference_delta: 1,        // Normal playback rate
        };
        // Current time of the system.
        let curr_time = 500000000;
        let song_position = media_timeline_fn_to_position(timeline_fn, curr_time);
        let expected_position = 0; //  -10000 + 0 = -10000 -> capped at 0.
        assert_eq!(song_position, Some(expected_position));
    }

    #[fuchsia::test]
    /// Tests conversion from Media RepeatMode to RepeatStatusMode
    fn test_media_repeat_mode_conversion() {
        let mode = fidl_media::RepeatMode::Off;
        assert_eq!(media_repeat_mode_to_avrcp(mode), fidl_avrcp::RepeatStatusMode::Off);
        let mode = fidl_media::RepeatMode::Group;
        assert_eq!(media_repeat_mode_to_avrcp(mode), fidl_avrcp::RepeatStatusMode::GroupRepeat);
        let mode = fidl_media::RepeatMode::Single;
        assert_eq!(
            media_repeat_mode_to_avrcp(mode),
            fidl_avrcp::RepeatStatusMode::SingleTrackRepeat
        );
    }

    #[fuchsia::test]
    /// Tests conversion from Media Shuffle flag to  ShuffleMode
    fn test_media_shuffle_mode_conversion() {
        assert_eq!(media_shuffle_mode_to_avrcp(true), fidl_avrcp::ShuffleMode::AllTrackShuffle);
        assert_eq!(media_shuffle_mode_to_avrcp(false), fidl_avrcp::ShuffleMode::Off);
    }

    #[fuchsia::test]
    /// Tests conversion from Media PlayerState to fidl_avrcp::PlaybackStatus
    fn test_media_player_state_conversion() {
        let state = fidl_media::PlayerState::Idle;
        assert_eq!(
            media_player_state_to_playback_status(state),
            fidl_avrcp::PlaybackStatus::Stopped
        );
        let state = fidl_media::PlayerState::Playing;
        assert_eq!(
            media_player_state_to_playback_status(state),
            fidl_avrcp::PlaybackStatus::Playing
        );
        let state = fidl_media::PlayerState::Paused;
        assert_eq!(
            media_player_state_to_playback_status(state),
            fidl_avrcp::PlaybackStatus::Paused
        );
        let state = fidl_media::PlayerState::Buffering;
        assert_eq!(
            media_player_state_to_playback_status(state),
            fidl_avrcp::PlaybackStatus::Paused
        );
        let state = fidl_media::PlayerState::Error;
        assert_eq!(media_player_state_to_playback_status(state), fidl_avrcp::PlaybackStatus::Error);
    }

    #[fuchsia::test]
    /// Tests getting only a specific event_id of a `Notification` success.
    fn test_notification_only_event_encoding() {
        let notif: Notification = fidl_avrcp::Notification {
            status: Some(fidl_avrcp::PlaybackStatus::Paused),
            track_id: Some(1000),
            pos: Some(99),
            battery_status: Some(fidl_avrcp::BatteryStatus::Critical),
            ..Default::default()
        }
        .into();

        // Supported event_id.
        let event_id = fidl_avrcp::NotificationEvent::TrackPosChanged;
        let expected: Notification =
            fidl_avrcp::Notification { pos: Some(99), ..Default::default() }.into();
        assert_eq!(expected, notif.only_event(&event_id));

        // Supported event_id, volume is None.
        let event_id = fidl_avrcp::NotificationEvent::VolumeChanged;
        let expected: Notification = fidl_avrcp::Notification::default().into();
        assert_eq!(expected, notif.only_event(&event_id));

        // Supported event_id.
        let event_id = fidl_avrcp::NotificationEvent::BattStatusChanged;
        let expected: Notification = fidl_avrcp::Notification {
            battery_status: Some(fidl_avrcp::BatteryStatus::Critical),
            ..Default::default()
        }
        .into();
        assert_eq!(expected, notif.only_event(&event_id));

        // Unsupported event_id.
        let event_id = fidl_avrcp::NotificationEvent::SystemStatusChanged;
        let expected: Notification = fidl_avrcp::Notification::default().into();
        assert_eq!(expected, notif.only_event(&event_id));
    }

    #[fuchsia::test]
    fn test_default_playback_rate_song_position() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(900000000));

        let playback_rate = PlaybackRate::default();
        let pos = playback_rate.current_position();
        assert_eq!(pos, Some(0));
    }
}
