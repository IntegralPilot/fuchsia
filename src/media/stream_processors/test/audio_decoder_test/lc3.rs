// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_media::*;
use stream_processor_test::*;

/// Represents an LC3 elementary stream.
pub struct Lc3Stream {
    data: Vec<u8>,
    oob_bytes: Vec<u8>,
    chunk_frames: usize,
}

impl Lc3Stream {
    /// Constructs a LC3 elementary stream from raw data.
    pub fn from_data(data: Vec<u8>, oob_bytes: Vec<u8>, chunk_frames: usize) -> Self {
        Lc3Stream { data, oob_bytes, chunk_frames }
    }
}

impl ElementaryStream for Lc3Stream {
    fn format_details(&self, version_ordinal: u64) -> FormatDetails {
        FormatDetails {
            format_details_version_ordinal: Some(version_ordinal),
            mime_type: Some(String::from("audio/lc3")),
            oob_bytes: Some(self.oob_bytes.clone()),
            ..FormatDetails::default()
        }
    }

    fn is_access_units(&self) -> bool {
        false
    }

    fn stream<'a>(&'a self) -> Box<dyn Iterator<Item = ElementaryStreamChunk> + 'a> {
        Box::new(self.data.chunks(self.chunk_frames).map(|frame| ElementaryStreamChunk {
            start_access_unit: false,
            known_end_access_unit: false,
            data: frame.to_vec(),
            significance: Significance::Audio(AudioSignificance::Encoded),
            timestamp: None,
        }))
    }
}
