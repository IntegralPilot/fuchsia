// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use anyhow::{Context as _, Error};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_media::*;
use fidl_fuchsia_media_sounds::*;
use fuchsia_async::{MonotonicInstant, Timer};
use fuchsia_component as component;
use futures::{FutureExt, join};
use std::fs::*;
use zerocopy::IntoBytes;
use zx::{self as zx, Vmo};

type Result<T> = std::result::Result<T, Error>;

#[fuchsia::main]
async fn main() -> Result<()> {
    let player_proxy = component::client::connect_to_protocol::<PlayerMarker>()
        .context("Connecting to fuchsia.media.sounds.Player")?;

    // If there are files in the isolated /tmp directory, play them and quit.
    if let Ok(tmpdir) = read_dir("/tmp") {
        let mut id = 0;
        let mut found_file = false;
        for entry in tmpdir {
            if let Ok(entry) = entry {
                if entry.path().is_file() {
                    found_file = true;
                    print!("{} ", entry.path().to_str().unwrap());
                    let duration = player_proxy
                        .add_sound_from_file(id, sound_file(entry.path().to_str().unwrap())?)
                        .await?
                        .map_err(|status| {
                            anyhow::format_err!("AddSoundFromFile failed {}", status)
                        })?;
                    print!("{:?}\n", std::time::Duration::from_nanos(duration.try_into().unwrap()));
                    player_proxy
                        .play_sound2(id, AudioRenderUsage2::Media)
                        .await?
                        .map_err(|err| anyhow::format_err!("PlaySound2 failed: {:?}", err))?;

                    id = id + 1;
                }
            }
        }

        if found_file {
            return Ok(());
        }
    }

    let file = resource_file("sfx.wav").await?;

    player_proxy
        .add_sound_from_file(0, file)
        .await?
        .map_err(|status| anyhow::format_err!("AddSoundFromFile failed {}", status))?;

    let duration = std::time::Duration::from_secs(1);

    let (buffer, stream_type) = sound_in_buffer(-8, 0.5, 0.9, duration)?;
    player_proxy.add_sound_buffer(1, buffer, &stream_type)?;

    let (buffer, stream_type) = sound_in_buffer(-17, 0.5, 0.9, duration)?;
    player_proxy.add_sound_buffer(2, buffer, &stream_type)?;

    let (buffer, stream_type) = sound_in_buffer(-12, 0.5, 0.9, duration)?;
    player_proxy.add_sound_buffer(3, buffer, &stream_type)?;

    // Play the file-based sound.
    player_proxy
        .play_sound2(0, AudioRenderUsage2::Media)
        .await?
        .map_err(|err| anyhow::format_err!("PlaySound2 failed: {:?}", err))?;

    // Play the VMO-based sounds in sequence.
    player_proxy
        .play_sound2(1, AudioRenderUsage2::Media)
        .await?
        .map_err(|err| anyhow::format_err!("PlaySound2 failed: {:?}", err))?;
    player_proxy
        .play_sound2(2, AudioRenderUsage2::Media)
        .await?
        .map_err(|err| anyhow::format_err!("PlaySound2 failed: {:?}", err))?;
    player_proxy
        .play_sound2(3, AudioRenderUsage2::Media)
        .await?
        .map_err(|err| anyhow::format_err!("PlaySound2 failed: {:?}", err))?;

    // Play the VMO-based sounds all at once.
    join!(
        player_proxy.play_sound2(1, AudioRenderUsage2::Media).map(|_| ()),
        player_proxy.play_sound2(2, AudioRenderUsage2::Media).map(|_| ()),
        player_proxy.play_sound2(3, AudioRenderUsage2::Media).map(|_| ())
    );

    // Play only 250ms of a sound by stopping it after a timer elapses.
    let timer = Timer::new(MonotonicInstant::after(zx::MonotonicDuration::from_millis(250)))
        .map(|_| player_proxy.stop_playing_sound(1));

    join!(player_proxy.play_sound2(1, AudioRenderUsage2::Media).map(|_| ()), timer.map(|_| ()));

    Ok(())
}

/// Creates a file channel from a resource file.
async fn resource_file(name: &str) -> Result<fidl::endpoints::ClientEnd<fio::FileMarker>> {
    use fidl::endpoints::Proxy as _;
    let path = format!("/pkg/data/{name}");
    let file = fuchsia_fs::file::open_in_namespace(&path, fuchsia_fs::PERM_READABLE)
        .with_context(|| format!("opening resource file: {path}"))?
        .into_client_end()
        .unwrap();
    Ok(file)
}

/// Creates a file channel from a file name.
fn sound_file(name: &str) -> Result<fidl::endpoints::ClientEnd<fio::FileMarker>> {
    let (client_end, server_end) = fidl::endpoints::create_endpoints();
    let () =
        fuchsia_fs::file::open_channel_in_namespace(name, fuchsia_fs::PERM_READABLE, server_end)?;
    Ok(client_end)
}

/// Creates a VMO-based sound containing a decaying sine wave using the slope iteration method.
/// `note` is in semitones with 0 being concert A (440Hz).
fn sound_in_buffer(
    note: i32,
    volume: f32,
    decay: f32,
    duration: std::time::Duration,
) -> Result<(fidl_fuchsia_mem::Buffer, AudioStreamType)> {
    const FRAMES_PER_SECOND: u32 = 44100;

    let frequency = frequency_from_note(note);
    let frame_count = (FRAMES_PER_SECOND as f32 * duration.as_secs_f32()) as usize;
    let rot_coeff = (2.0 * std::f32::consts::PI * frequency) / (FRAMES_PER_SECOND as f32);
    let decay_factor = (1.0 - decay).powf(1.0 / FRAMES_PER_SECOND as f32);

    let mut real_sample: f32 = 0.0;
    let mut imaginary_sample: f32 = 32760.0;
    let mut v = volume;

    let mut samples = std::vec::Vec::with_capacity(frame_count);
    for _i in 0..frame_count {
        samples.push((real_sample * v) as i16);

        // Rotate real_sample,imaginary_sample around the origin.
        real_sample -= imaginary_sample * rot_coeff;
        imaginary_sample += real_sample * rot_coeff;

        v = v * decay_factor;
    }

    let vmo = Vmo::create((frame_count * 2) as u64).context("Creating VMO")?;
    vmo.write(&samples.as_bytes(), 0).context("Writing to VMO")?;

    Ok((
        fidl_fuchsia_mem::Buffer { vmo: vmo, size: (frame_count * 2).try_into()? },
        AudioStreamType {
            sample_format: AudioSampleFormat::Signed16,
            channels: 1,
            frames_per_second: FRAMES_PER_SECOND,
        },
    ))
}

/// Calculates frequency from a note number. We multiply concert A frequency (440Hz) by the
/// twelfth root of two to the 'note' power.
fn frequency_from_note(note: i32) -> f32 {
    const CONCERT_A_FREQUENCY: f32 = 440.0;
    return CONCERT_A_FREQUENCY * 2.0_f32.powf(note as f32 / 12.0);
}
