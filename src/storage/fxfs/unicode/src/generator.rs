// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use ::types::{CCCEntry, MappingOffset};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::ops::Range;

mod ucd_parsers {
    use regex::Regex;
    use std::collections::BTreeMap;
    use std::io::Read;
    use zip::read::ZipArchive;

    pub fn read_zip(zip_path: &str, path: &str) -> Result<String, std::io::Error> {
        let mut zip = ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
        let mut file = zip.by_name(path).map_err(|_| std::io::ErrorKind::NotFound)?;
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        Ok(String::from_utf8(out).unwrap())
    }

    /// Parse and return CaseFolding.txt data from UCD.
    pub fn case_folding(ucd_zip: &str) -> Result<BTreeMap<u32, Vec<u32>>, std::io::Error> {
        let mut out = BTreeMap::new();
        let re = Regex::new(r"([A-F0-9]+); (.); ([A-F0-9]+(?: [A-F0-9]+)*); .*$").unwrap();
        for line in read_zip(ucd_zip, "CaseFolding.txt")?.split("\n") {
            if let Some(cap) = re.captures(&line) {
                let code = cap.get(1).unwrap().as_str();
                let label = cap.get(2).unwrap().as_str();
                let mapping = cap.get(3).unwrap().as_str();
                if label == "C" || label == "F" {
                    let code = u32::from_str_radix(code, 16).unwrap();
                    let mapping: Vec<u32> =
                        mapping.split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
                    out.insert(code, mapping);
                }
            }
        }
        Ok(out)
    }

    /// Combining classes (CCC) are used to normalize the order of modifier codepoints.
    pub fn derived_combining_class(ucd_zip: &str) -> Result<Vec<(u32, u32, u8)>, std::io::Error> {
        let mut ccc = Vec::new();
        let re = Regex::new(r"([A-F0-9]+)(?:\.\.([A-F0-9]+))? +; ([0-9]+) #.*$").unwrap();
        for line in read_zip(ucd_zip, "extracted/DerivedCombiningClass.txt")?.split("\n") {
            if let Some(cap) = re.captures(&line) {
                let start = u32::from_str_radix(&cap[1], 16).unwrap();
                let end = if cap.get(2).is_none() {
                    start
                } else {
                    u32::from_str_radix(&cap[2], 16).unwrap()
                } + 1;
                let v = cap[3].parse::<u8>().unwrap();
                ccc.push((start, end, v));
            }
        }
        ccc.sort();
        Ok(ccc)
    }

    /// Extracts the default ignorable code points from DerivedCoreProperties.txt into a map.
    pub fn default_ignorable_code_point(ucd_zip: &str) -> Result<Vec<(u32, u32)>, std::io::Error> {
        let mut ranges = Vec::new();
        let re =
            Regex::new(r"([A-F0-9]+)(?:\.\.([A-F0-9]+))? *; Default_Ignorable_Code_Point #.*$")
                .unwrap();
        for line in read_zip(ucd_zip, "DerivedCoreProperties.txt")?.split("\n") {
            if let Some(cap) = re.captures(&line) {
                let start = u32::from_str_radix(&cap[1], 16).unwrap();
                let end = if cap.get(2).is_none() {
                    start
                } else {
                    u32::from_str_radix(&cap[2], 16).unwrap()
                } + 1;
                ranges.push((start, end));
            }
        }
        ranges.sort();
        Ok(ranges)
    }

    /// This holds a subset of the data stored in a row of UnicodeData.txt.
    /// We only decode and keep fields that we plan to use.
    #[derive(Default, Debug, Eq, PartialEq)]
    pub struct UnicodeData {
        /// The name of the character as published in Chapter 7 of the Unicode standard.
        pub character_name: String,
        /// A set of two-byte categories for this character, split into normative and informative.
        pub general_category: String,
        /// Canonical combining classes.
        pub ccc: u32,
        /// Note: Decomposition must be done recursively for maximal decomposition.
        pub character_decomposition: Vec<u32>,
        /// The uppercase equivalent mapping (informative)
        pub uppercase: Option<u32>,
        /// The lowrecase equivalent mapping (informative)
        pub lowercase: Option<u32>,
    }
    impl UnicodeData {
        pub fn from_raw(raw: Vec<&str>) -> (u32, Self) {
            let codepoint = u32::from_str_radix(&raw[0], 16).unwrap();
            let character_name = raw[1].to_owned();
            let general_category = raw[2].to_owned();
            let ccc = raw[3].parse::<u32>().unwrap();
            // Character decompositions sometimes have tags like '<noBreak> ' prefixed.
            // The presence of a tag implies the mapping is not canonical so we should ignore it.
            let character_decomposition: Vec<u32> = if !raw[5].is_empty() && &raw[5][..1] != "<" {
                raw[5].split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect()
            } else {
                vec![]
            };
            let uppercase = u32::from_str_radix(&raw[12], 16).ok();
            let lowercase = u32::from_str_radix(&raw[13], 16).ok();
            (
                codepoint,
                Self {
                    character_name,
                    general_category,
                    ccc,
                    character_decomposition,
                    uppercase,
                    lowercase,
                },
            )
        }
    }

    /// Cherry picked data from 'UnicodeData.txt' provided as a map.
    pub fn unicode_data(ucd_zip: &str) -> Result<BTreeMap<u32, UnicodeData>, std::io::Error> {
        let mut data = BTreeMap::new();
        for line in read_zip(ucd_zip, "UnicodeData.txt")?.split("\n") {
            if line.len() == 0 {
                continue;
            }
            let (key, val) = UnicodeData::from_raw(line.split(";").collect());
            data.insert(key, val);
        }
        Ok(data)
    }

    #[cfg(test)]
    mod test {
        use super::*;

        fn get_icu_dir() -> String {
            let fuchsia_dir = std::env::vars().find(|(k, _)| k == "FUCHSIA_DIR").unwrap().1;
            format!("{fuchsia_dir}/third_party/icu/latest/source/data/unidata")
        }

        #[test]
        fn test_types() {
            let entry = CCCEntry { low_plane_end_ix: 10, range_len: 5, ccc: 1 };
            assert_eq!(entry.range(), 5..10);
        }

        #[test]
        fn test_derived_combining_class() {
            let ccc = derived_combining_class(&get_icu_dir()).unwrap();
            assert_eq!(ccc[0], (0, 32, 0));
        }

        #[test]
        fn test_unicode_data() {
            let data = unicode_data(&get_icu_dir()).unwrap();
            // This is just a token check that expected data exists.
            assert_eq!(
                data[&0x20],
                UnicodeData {
                    character_name: "SPACE".to_owned(),
                    general_category: "Zs".to_owned(),
                    ..UnicodeData::default()
                }
            );
            assert_eq!(
                data[&0xf900],
                UnicodeData {
                    character_name: "CJK COMPATIBILITY IDEOGRAPH-F900".to_owned(),
                    general_category: "Lo".to_owned(),
                    character_decomposition: vec![35912],
                    ..UnicodeData::default()
                }
            );
        }
    }
}

/// Generates 'unicode_gen.rs', a compact subset of the UCD dataset that can be used for
/// unicode NFD normalization, case folding and default_ignorable_code_point removal.
///
/// To use this dataset, see the fxfs_unicode crate.
pub fn unicode_gen(ucd_zip: &str, dest_path: &str) {
    let mut total_bytes = 0;

    let mut file = std::io::BufWriter::new(std::fs::File::create(dest_path).unwrap());

    write!(
        &mut file,
        "// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file was auto-generated from //prebuilt/third_party/ucd by
// //src/storage/fxfs/unicode/src/generator.rs
//
// ** DO NOT EDIT **
use ::types::{{CCCEntry, MappingOffset}};
use std::ops::Range;
"
    )
    .unwrap();

    // CCC is looked up for every character so we need this to be pretty fast but also pretty
    // compact. The approach taken here is a two-level indirection to reduce the code-point from
    // 32-bit (21-bit usable) to 16-bit and then a binary search. Total data set size is ~1.5kB
    // and likely to cache well.
    {
        let mut planes: Vec<Vec<CCCEntry>> = Vec::new();
        for _ in 0..18 {
            planes.push(Vec::new());
        }
        for (start, end, ccc) in ucd_parsers::derived_combining_class(ucd_zip).unwrap() {
            assert_eq!(
                start >> 16,
                end >> 16,
                "Start and end are in different planes for {start} and {end}"
            );
            if ccc != 0 {
                let range_len = end - start;
                assert!(range_len <= 255, "CCC data contains ranges too big to fit in one byte.");
                let plane = (end as usize) >> 16;
                planes[plane].push(CCCEntry {
                    low_plane_end_ix: end as u16,
                    range_len: range_len as u8,
                    ccc,
                });
            }
        }
        let mut plane_offsets: Vec<usize> = vec![0];
        let mut plane_data: Vec<CCCEntry> = Vec::new();
        for i in 0..planes.len() {
            plane_data.append(&mut planes[i]);
            plane_offsets.push(plane_data.len());
        }
        write!(&mut file, "pub static CCC_PLANE_OFFSETS : &[usize] = &{plane_offsets:?};\n")
            .unwrap();
        write!(&mut file, "pub static CCC_PLANE_DATA : &[CCCEntry] = &{plane_data:?};\n").unwrap();
        assert_eq!(std::mem::size_of::<CCCEntry>(), 4, "Unexpected size.");
        total_bytes += plane_data.len() * std::mem::size_of::<CCCEntry>() + plane_offsets.len() * 8;
    }

    // Recursive decomposition can lead to n characters for any given code point.
    // We use a similar approach to CCC above to reduce characters down to 2-bytes, but the data
    // stored is the map a u16 offset into DECOMP_STRINGS instead of a CCC and length.
    //
    // These strings are stored back-to-back. The end of one string is found by looking at the
    // offset of the string for the next character.
    {
        let unicode_data = ucd_parsers::unicode_data(ucd_zip).unwrap();
        let mut strings: Vec<u8> = Vec::new();
        let mut ch_to_offset = BTreeMap::new();

        for (&ch, data) in &unicode_data {
            if !data.character_decomposition.is_empty() {
                let mut chars = VecDeque::new();
                chars.push_front(ch);
                let mut out = String::new();
                while let Some(ch) = chars.pop_front() {
                    if let Some(data) = unicode_data.get(&ch) {
                        if data.character_decomposition.is_empty() {
                            out.push(char::from_u32(ch).unwrap());
                        } else {
                            for ch in data.character_decomposition.iter().rev() {
                                chars.push_front(*ch);
                            }
                        }
                    } else {
                        out.push(char::from_u32(ch).unwrap());
                    }
                }
                ch_to_offset.insert(ch, strings.len());
                strings.extend(out.as_bytes());
            }
        }
        write!(&mut file, "pub static DECOMP_STRINGS : &[u8] = &{strings:?};\n").unwrap();
        total_bytes += strings.len();

        let mut planes: Vec<Vec<MappingOffset>> = Vec::new();
        for _ in 0..18 {
            planes.push(Vec::new());
        }
        for (ch, offset) in ch_to_offset {
            assert!(offset <= u16::MAX as usize);
            let plane = (ch as usize) >> 16;
            planes[plane].push(MappingOffset { low_plane_ix: ch as u16, offset: offset as u16 });
        }
        let mut plane_offsets: Vec<usize> = vec![0];
        let mut plane_data: Vec<MappingOffset> = Vec::new();
        for i in 0..planes.len() {
            plane_data.append(&mut planes[i]);
            plane_offsets.push(plane_data.len());
        }
        write!(&mut file, "pub static DECOMP_PLANE_OFFSETS : &[usize] = &{plane_offsets:?};\n")
            .unwrap();
        write!(&mut file, "pub static DECOMP_PLANE_DATA : &[MappingOffset] = &{plane_data:?};\n")
            .unwrap();
        assert_eq!(std::mem::size_of::<MappingOffset>(), 4, "Unexpected size.");
        total_bytes +=
            plane_data.len() * std::mem::size_of::<MappingOffset>() + plane_offsets.len() * 8;
    }

    {
        let mut planes: Vec<Vec<Range<u16>>> = Vec::new();
        for _ in 0..18 {
            planes.push(Vec::new());
        }
        for (start, end) in ucd_parsers::default_ignorable_code_point(ucd_zip).unwrap() {
            let plane = (end as usize) >> 16;
            planes[plane].push(start as u16..end as u16);
        }
        let mut plane_offsets: Vec<usize> = vec![0];
        let mut plane_data: Vec<Range<u16>> = Vec::new();
        for i in 0..planes.len() {
            plane_data.append(&mut planes[i]);
            plane_offsets.push(plane_data.len());
        }
        write!(&mut file, "pub static IGNORABLE_PLANE_OFFSETS : &[usize] = &{plane_offsets:?};\n")
            .unwrap();
        write!(&mut file, "pub static IGNORABLE_PLANE_DATA : &[Range<u16>] = &{plane_data:?};\n")
            .unwrap();
        assert_eq!(std::mem::size_of::<Range<u16>>(), 4, "Unexpected size");
        total_bytes +=
            plane_data.len() * std::mem::size_of::<Range<u16>>() + plane_offsets.len() * 8;
    }

    {
        let casefold = ucd_parsers::case_folding(ucd_zip).unwrap();
        let mut strings: Vec<u8> = Vec::new();
        let mut ch_to_offset = BTreeMap::new();

        for (&ch, mapping) in &casefold {
            let mapping: String = mapping.iter().map(|x| char::from_u32(*x).unwrap()).collect();
            ch_to_offset.insert(ch, strings.len());
            strings.extend(mapping.as_bytes());
        }
        write!(&mut file, "pub static CASEFOLD_STRINGS : &[u8] = &{strings:?};\n").unwrap();
        total_bytes += strings.len();

        let mut planes: Vec<Vec<MappingOffset>> = Vec::new();
        for _ in 0..18 {
            planes.push(Vec::new());
        }
        for (ch, offset) in ch_to_offset {
            let plane = (ch as usize) >> 16;
            planes[plane].push(MappingOffset { low_plane_ix: ch as u16, offset: offset as u16 });
        }
        let mut plane_offsets: Vec<usize> = vec![0];
        let mut plane_data: Vec<MappingOffset> = Vec::new();
        for i in 0..planes.len() {
            plane_data.append(&mut planes[i]);
            plane_offsets.push(plane_data.len());
        }
        write!(&mut file, "pub static CASEFOLD_PLANE_OFFSETS : &[usize] = &{plane_offsets:?};\n")
            .unwrap();
        write!(&mut file, "pub static CASEFOLD_PLANE_DATA : &[MappingOffset] = &{plane_data:?};\n")
            .unwrap();
        assert_eq!(std::mem::size_of::<MappingOffset>(), 4, "Unexpected size.");
        total_bytes +=
            plane_data.len() * std::mem::size_of::<MappingOffset>() + plane_offsets.len() * 8;
    }

    assert!(total_bytes < 30000, "validity check - data set is not too large");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <UCD.zip path> <output.rs>", args[0]);
    } else {
        unicode_gen(&args[1], &args[2]);
    }
}
