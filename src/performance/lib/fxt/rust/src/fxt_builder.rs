// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use thiserror::Error;

#[derive(Clone)]
pub struct FxtBuilder<H> {
    header: H,
    buf: Vec<u8>,
}

#[derive(Error, Debug)]
pub enum SerializeError {
    #[error("Encountered Empty StringRefs when serializing argument's name")]
    MissingArgName,
}

impl<H: crate::header::TraceHeader> FxtBuilder<H> {
    /// Start a new fxt record with a typed header. The header should be completely configured for
    /// the corresponding record except for its size in words which will be updated by the builder.
    pub fn new(mut header: H) -> Self {
        // Make space for our header word before anything gets added.
        let buf = vec![0; 8];

        // Set an initial size, we'll update as we go.
        header.set_size_words(1);

        Self { header, buf }
    }

    pub fn atom(mut self, atom: impl AsRef<[u8]>) -> Self {
        self.buf.extend(atom.as_ref());
        for _ in 0..crate::word_padding(self.buf.len()) {
            self.buf.push(0);
        }
        assert_eq!(self.buf.len() % 8, 0, "buffer should be word-aligned after adding padding");
        assert!(self.buf.len() < 32_768, "maximum record size is 32kb");
        let size_words: u16 =
            (self.buf.len() / 8).try_into().expect("trace records size in words must fit in a u16");
        self.header.set_size_words(size_words);
        self
    }

    /// Return the bytes of a possibly-valid fxt record with the header in place.
    pub fn build(mut self) -> Vec<u8> {
        self.buf[..8].copy_from_slice(&self.header.to_le_bytes());
        self.buf
    }
}

impl<H: std::fmt::Debug> std::fmt::Debug for FxtBuilder<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Print in word-aligned chunks, exclude the zeroes we keep for the header.
        let chunks = self.buf.chunks_exact(8).skip(1).collect::<Vec<_>>();
        f.debug_struct("FxtBuilder").field("header", &self.header).field("buf", &chunks).finish()
    }
}
