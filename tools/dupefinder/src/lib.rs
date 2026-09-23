// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flate2::read::GzDecoder;
use handlebars::{Handlebars, handlebars_helper};
use log::debug;
use pprof::Profile as RawProfile;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use thiserror::Error;

static REPORT_TEMPLATE: &str = include_str!("../templates/report.html.hbs");

#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize)]
pub struct Sample {
    metadata: Metadata,
    stack_trace: Vec<Location>,
}

impl Sample {
    pub fn parse(profile_bytes: &[u8]) -> Result<Vec<Self>, DupefinderError> {
        // pprof profiles are often gzipped; handle both compressed and raw profiles transparently.
        let mut decoded_bytes = Vec::new();
        let mut decoder = GzDecoder::new(profile_bytes);
        let profile_bytes = if decoder.read_to_end(&mut decoded_bytes).is_ok() {
            &decoded_bytes
        } else {
            profile_bytes
        };

        let raw_profile =
            RawProfile::decode(profile_bytes).map_err(DupefinderError::DecodeRawProfileFailed)?;

        let functions =
            raw_profile.function.iter().map(|f| (f.id, f.clone())).collect::<HashMap<_, _>>();

        let mappings =
            raw_profile.mapping.iter().map(|m| (m.id, m.clone())).collect::<HashMap<_, _>>();

        let mut locations = HashMap::new();
        for loc in &raw_profile.location {
            let mut lines = vec![];
            for line in &loc.line {
                let function_id = &(line.function_id as u64);
                lines.push(SourceLine {
                    function: raw_profile.string_table[functions[function_id].name as usize]
                        .clone(),
                    filename: raw_profile.string_table[functions[function_id].filename as usize]
                        .clone(),
                    line: line.line,
                });
            }

            let library_name = mappings
                .get(&loc.mapping_id)
                .map(|m| raw_profile.string_table[m.filename as usize].to_owned());

            locations.insert(
                loc.id,
                Location { address: loc.address, library_name: library_name, lines },
            );
        }

        let mut samples = vec![];

        for s in &raw_profile.sample {
            let mut address = None;
            let mut size = None;
            let mut timestamp = None;
            for label in &s.label {
                let key = &raw_profile.string_table[label.key as usize];
                match key.as_str() {
                    "address" => {
                        address = Some(raw_profile.string_table[label.str as usize].clone())
                    }
                    "bytes" => size = Some(label.num),
                    "timestamp" => timestamp = Some(label.num),

                    // Ignore any unknown labels to avoid lockstep updates if they don't impact
                    // the analysis here.
                    _ => (),
                }
            }

            samples.push(Sample {
                metadata: Metadata {
                    address: address.unwrap(),
                    size: size.unwrap(),
                    timestamp: timestamp.unwrap(),
                },
                stack_trace: s.location_id.iter().map(|id| locations[id].clone()).collect(),
            });
        }

        Ok(samples)
    }

    fn first_blame_candidate(&self) -> Option<&SourceLine> {
        self.stack_trace.iter().flat_map(|l| l.first_blame_candidate()).next()
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DupesReport {
    /// Duplicated bytes per blamed source location.
    dupe_locations: Vec<Duplication>,

    // NOTE: remaining fields are for serializing to handlebars
    dupe_threshold: usize,
    timestamp: String,

    // allocation counts
    num_total: usize,
    num_uniques: usize,
    num_dupes: usize,

    // size tallies
    unique_bytes: usize,
    duped_bytes: usize,
    unblamed_duped_bytes: usize,
    total_bytes: usize,
}

impl DupesReport {
    pub fn new(
        heap_contents: &HashMap<String, Vec<u8>>,
        samples: Vec<Sample>,
        max_allocs_reported: usize,
        dupe_threshold: usize,
        timestamp: String,
    ) -> Result<Self, DupefinderError> {
        // get the contents for each symbolized sample
        let mut samples_by_contents: HashMap<Vec<u8>, Vec<Sample>> = HashMap::new();
        for sample in samples {
            let Some(contents) = heap_contents.get(&sample.metadata.address) else {
                // This allocation was probably freed after the heap metadata was snapshotted but
                // before the address' contents were recorded.
                continue;
            };

            if sample.metadata.size as usize != contents.len() {
                return Err(DupefinderError::SizeMismatch {
                    size_on_disk: contents.len(),
                    size_in_metadata: sample.metadata.size as usize,
                });
            }

            samples_by_contents.entry(contents.to_owned()).or_default().push(sample);
        }

        // split allocations into duplicated vs. unique
        let mut duped_allocations = vec![];
        let mut unique_allocations = vec![];
        for (contents, samples) in samples_by_contents {
            let alloc = Allocation::new(contents, samples)?;
            if alloc.num_copies() > 1 {
                duped_allocations.push(alloc);
            } else {
                unique_allocations.push(alloc);
            }
        }

        // analyze code locations which cause duplication
        let mut dupe_sources: HashMap<&SourceLine, usize> = Default::default();
        for a in &duped_allocations {
            for (count, line) in a.blamed_lines() {
                *dupe_sources.entry(line).or_default() += a.single_bytes() * count;
            }
        }
        // store the blamed locations sorted by frequency
        let mut dupe_locations = dupe_sources
            .into_iter()
            .filter(|(_, count)| count >= &dupe_threshold)
            .map(|(location, bytes)| Duplication { bytes, source: location.clone() })
            .collect::<Vec<_>>();
        dupe_locations.sort();
        dupe_locations.reverse();
        dupe_locations.truncate(max_allocs_reported);

        // collect counts to summarize

        // one for each unique allocation and one for the initial alloc of each dupe
        let num_uniques = unique_allocations.len() + duped_allocations.len();
        // don't count the initial allocation for each value in the duplicates
        let num_dupes = duped_allocations.iter().map(|a| a.num_copies() - 1).sum();
        let num_total = num_uniques + num_dupes;

        // tally unique allocations
        let no_dupes: usize = unique_allocations.iter().map(|a| a.total_bytes()).sum();
        let first_of_each_dupe: usize = duped_allocations.iter().map(|a| a.single_bytes()).sum();
        let unique_bytes = no_dupes + first_of_each_dupe;

        // don't count the initial allocation for each value in the duplicates
        let unblamed_duped_bytes = duped_allocations.iter().map(|a| a.unblamed_bytes()).sum();
        let duped_bytes =
            duped_allocations.iter().map(|a| a.total_bytes() - a.single_bytes()).sum();
        let total_bytes = unique_bytes + duped_bytes;

        Ok(Self {
            dupe_locations,
            dupe_threshold,
            timestamp,
            num_uniques,
            num_dupes,
            num_total,
            unique_bytes,
            duped_bytes,
            unblamed_duped_bytes,
            total_bytes,
        })
    }

    pub fn dupe_locations(&self) -> &[Duplication] {
        &self.dupe_locations
    }

    pub fn summarize(&self) {
        debug!("Summary:");
        debug!("# allocations: {}", self.num_total);
        debug!("# unique allocations: {}", self.num_uniques);
        debug!("# duplicate allocations: {}", self.num_dupes);
        debug!("# source locations with duplicates: {}", self.dupe_locations.len());
        debug!("Unique allocated bytes: {}", self.unique_bytes);
        debug!("Duplicate allocated bytes: {}", self.duped_bytes);
        debug!("Total allocation bytes: {}", self.total_bytes);
    }

    pub fn write_report(&self, f: impl Write) -> Result<(), DupefinderError> {
        let mut hbs = Handlebars::new();
        hbs.set_strict_mode(true); // surface errors if templates don't render correctly
        hbs.set_prevent_indent(true); // nested templates should inherit indentation
        hbs.register_template_string("report", REPORT_TEMPLATE)
            .map_err(Box::new)
            .map_err(DupefinderError::HandlebarsSetupFailed)?;
        hbs.register_helper("codesearch_url", Box::new(codesearch_url));
        hbs.render_to_write("report", self, f).map_err(DupefinderError::RenderHtmlFailed)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq, PartialOrd, Ord, Serialize)]
pub struct Duplication {
    bytes: usize,
    source: SourceLine,
}

impl Duplication {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn source(&self) -> &SourceLine {
        &self.source
    }
}

/// A group of allocations with identical heap contents.
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize)]
struct Allocation {
    contents: Vec<u8>,
    samples: Vec<Sample>,

    /// Lines which referenced this duplicated value, with a count for how many times seen.
    blame: Vec<(usize, SourceLine)>,

    /// Times this value was created by code without an apparent in-tree source reference on the
    /// stack.
    unblamed: usize,
}

impl Allocation {
    fn new(contents: Vec<u8>, samples: Vec<Sample>) -> Result<Self, DupefinderError> {
        if samples.is_empty() {
            return Err(DupefinderError::NoSamples);
        }

        let mut blame_counts: HashMap<_, usize> = HashMap::new();
        let mut unblamed = 0;
        for s in &samples {
            if let Some(candidate) = s.first_blame_candidate() {
                *blame_counts.entry(candidate).or_default() += 1;
            } else {
                unblamed += 1;
            }
        }

        let mut blame = blame_counts
            .into_iter()
            .map(|(location, count)| (count, location.clone()))
            .collect::<Vec<_>>();
        blame.sort();
        blame.reverse();

        Ok(Self { contents, samples, blame, unblamed })
    }

    fn blamed_lines(&self) -> &[(usize, SourceLine)] {
        &self.blame
    }

    fn num_copies(&self) -> usize {
        self.samples.len()
    }

    fn single_bytes(&self) -> usize {
        self.contents.len()
    }

    fn unblamed_bytes(&self) -> usize {
        self.unblamed * self.single_bytes()
    }

    fn total_bytes(&self) -> usize {
        self.num_copies() * self.single_bytes()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize)]
struct Metadata {
    address: String,
    size: i64,
    timestamp: i64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize)]
struct Location {
    address: u64,
    library_name: Option<String>,
    lines: Vec<SourceLine>,
}

impl Location {
    fn first_blame_candidate(&self) -> Option<&SourceLine> {
        self.lines.iter().filter(|l| l.is_candidate_for_blame()).next()
    }
}

#[derive(Clone, Deserialize, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize)]
pub struct SourceLine {
    filename: String,
    line: i64,
    function: String,
}

impl SourceLine {
    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn line(&self) -> i64 {
        self.line
    }

    pub fn function(&self) -> &str {
        &self.function
    }

    fn is_candidate_for_blame(&self) -> bool {
        // ignore the allocator hook itself
        if self.function.contains("__scudo_allocate_hook")
            || self.function.contains("__scudo_realloc_allocate_hook")
            || self.function.contains("_core_rustc_static::hooks::with_profiler_and_call_site")
            || self.function.contains("__sanitizer_fast_backtrace")
            || self.function.contains("zx_libc::sanitizer::fast_backtrace")
        {
            return false;
        }

        // ignore external code
        if self.filename.starts_with("../../third_party") {
            return false;
        }

        // ignore prebuilts
        self.filename.starts_with("../../")
    }
}

handlebars_helper!(codesearch_url: |filename: String, line: i64| {
    use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
    let filename = filename.strip_prefix("../../").unwrap_or(&filename);
    format!(
        "https://cs.opensource.google/search?q=file:{}&sq=&ss=fuchsia",
        percent_encode(format!("{filename}:{line}").as_bytes(), NON_ALPHANUMERIC),
    )
});

pub fn read_heap_contents_dir(
    path: impl AsRef<Path>,
) -> Result<HashMap<String, Vec<u8>>, std::io::Error> {
    let path = path.as_ref();
    let mut heap_contents = HashMap::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let address = entry.file_name().to_string_lossy().to_string();
        let alloc_contents = std::fs::read(entry.path())?;
        heap_contents.insert(address, alloc_contents);
    }
    Ok(heap_contents)
}

#[derive(Debug, Error)]
pub enum DupefinderError {
    #[error("couldn't decode raw profile")]
    DecodeRawProfileFailed(#[source] prost::DecodeError),
    #[error("profile says {size_in_metadata} bytes but on disk it is {size_on_disk}")]
    SizeMismatch { size_on_disk: usize, size_in_metadata: usize },
    #[error("profile had no samples")]
    NoSamples,
    #[error("heap contents didn't include {address}")]
    MissingAddress { address: String },
    #[error("encountered unknown label on heapdump sample `{unknown}`")]
    UnknownSampleLabel { unknown: String },
    #[error("couldn't configure handlebars template")]
    HandlebarsSetupFailed(#[source] Box<handlebars::TemplateError>),
    #[error("couldn't render HTML")]
    RenderHtmlFailed(#[source] handlebars::RenderError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use pprof::{Label as RawLabel, Sample as RawSample};

    #[test]
    fn test_gzipped_profile_fallback() {
        const FAKE_SAMPLE_ADDRESS: &str = "0x1234";
        const FAKE_SAMPLE_SIZE: i64 = 100;
        const FAKE_SAMPLE_TIMESTAMP: i64 = 1000;

        // Create a minimal profile.
        let mut string_table = pprof::StringTableBuilder::default();
        let sample = vec![RawSample {
            label: vec![
                RawLabel {
                    key: string_table.intern("address"),
                    str: string_table.intern(FAKE_SAMPLE_ADDRESS),
                    ..Default::default()
                },
                RawLabel {
                    key: string_table.intern("bytes"),
                    num: FAKE_SAMPLE_SIZE,
                    ..Default::default()
                },
                RawLabel {
                    key: string_table.intern("timestamp"),
                    num: FAKE_SAMPLE_TIMESTAMP,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }];
        let profile =
            RawProfile { string_table: string_table.build(), sample, ..Default::default() };

        // Encode to protobuf.
        let mut profile_bytes = Vec::new();
        profile.encode(&mut profile_bytes).unwrap();

        // Compress it.
        let mut compressed_bytes = Vec::new();
        let mut encoder = GzEncoder::new(&mut compressed_bytes, Compression::default());
        encoder.write_all(&profile_bytes).unwrap();
        encoder.finish().unwrap();

        // Verify that it can be read back from the compressed form.
        let samples = Sample::parse(&compressed_bytes).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].metadata.address, FAKE_SAMPLE_ADDRESS);
        assert_eq!(samples[0].metadata.size, FAKE_SAMPLE_SIZE);
        assert_eq!(samples[0].metadata.timestamp, FAKE_SAMPLE_TIMESTAMP);

        // Verify that it can be read back from the uncompressed form too.
        let samples = Sample::parse(&profile_bytes).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].metadata.address, FAKE_SAMPLE_ADDRESS);
        assert_eq!(samples[0].metadata.size, FAKE_SAMPLE_SIZE);
        assert_eq!(samples[0].metadata.timestamp, FAKE_SAMPLE_TIMESTAMP);
    }
}
