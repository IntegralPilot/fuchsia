// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_media::*;
use std::path::Path;
use std::{fs, io};
use stream_processor_test::*;

/// Represents an CVSD elementary stream.
pub struct CvsdStream {
    data: Vec<u8>,
    chunk_frames: usize,
}

impl CvsdStream {
    /// Constructs an CVSD elementary stream from a file with raw elementary stream data.
    pub fn from_file(filename: impl AsRef<Path>, chunk_frames: usize) -> io::Result<Self> {
        Ok(CvsdStream { data: fs::read(filename)?, chunk_frames })
    }
    /// Constructs an CVSD elementary stream from raw data.
    pub fn from_data(data: Vec<u8>, chunk_frames: usize) -> Self {
        CvsdStream { data, chunk_frames }
    }
}

impl ElementaryStream for CvsdStream {
    fn format_details(&self, version_ordinal: u64) -> FormatDetails {
        FormatDetails {
            format_details_version_ordinal: Some(version_ordinal),
            mime_type: Some(String::from("audio/cvsd")),
            ..Default::default()
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
