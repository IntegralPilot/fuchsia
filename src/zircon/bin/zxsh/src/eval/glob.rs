// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sort::quick_sort;
use bstr::{BStr, BString, ByteSlice};
use std::os::unix::ffi::OsStrExt;

/// Represents a single byte in a word being evaluated for glob matching or expansion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WordChar {
    /// A quoted byte (e.g., from inside quotes or escaped with backslash), never treated as a
    /// wildcard.
    Quoted(u8),
    /// An unquoted literal byte from the original script text.
    Unquoted(u8),
    /// A byte resulting from parameter or arithmetic expansion.
    Expansion(u8),
}

impl WordChar {
    /// Returns `true` if this is an unquoted or expanded byte matching `target`.
    pub fn is_wildcard(&self, target: u8) -> bool {
        match self {
            WordChar::Unquoted(c) | WordChar::Expansion(c) => *c == target,
            WordChar::Quoted(_) => false,
        }
    }

    /// Returns `true` if this byte should be treated as any glob wildcard syntax (`*`, `?`, or
    /// `[`).
    pub fn is_glob_wildcard(&self) -> bool {
        self.is_wildcard(b'*') || self.is_wildcard(b'?') || self.is_wildcard(b'[')
    }

    /// Returns `true` if this character is quoted.
    pub fn is_quoted(&self) -> bool {
        matches!(self, WordChar::Quoted(_))
    }

    /// Returns the underlying raw byte value regardless of quoting or expansion status.
    pub fn raw_byte(&self) -> u8 {
        match self {
            WordChar::Unquoted(c) | WordChar::Expansion(c) | WordChar::Quoted(c) => *c,
        }
    }

    /// Returns `true` if this character resulted from expansion and is an IFS whitespace byte.
    pub fn is_ifs_whitespace(&self, ifs: &BStr) -> bool {
        match self {
            WordChar::Expansion(c) => (*c == b' ' || *c == b'\t' || *c == b'\n') && ifs.contains(c),
            _ => false,
        }
    }

    /// Returns `true` if this character resulted from expansion and is an IFS non-whitespace byte.
    pub fn is_ifs_non_whitespace(&self, ifs: &BStr) -> bool {
        match self {
            WordChar::Expansion(c) => {
                !(*c == b' ' || *c == b'\t' || *c == b'\n') && ifs.contains(c)
            }
            _ => false,
        }
    }
}

pub fn word_chars_to_bstring(word: &[WordChar]) -> BString {
    let mut bytes = Vec::with_capacity(word.len());
    for wc in word {
        bytes.push(wc.raw_byte());
    }
    BString::from(bytes)
}

/// Checks whether a byte string pattern (treated as unquoted characters) matches the target text.
///
/// Supports POSIX shell wildcards: `*` (any sequence), `?` (any single byte), and bracket
/// expressions `[...]`.
pub fn match_glob(pattern: &BStr, text: &BStr) -> bool {
    let mut chars = Vec::with_capacity(pattern.len());
    for &b in pattern.as_bytes() {
        chars.push(WordChar::Unquoted(b));
    }
    match_segment_glob(&chars, text)
}

fn match_char_class(target_byte: u8, class_name: &str) -> bool {
    match class_name {
        "alnum" => target_byte.is_ascii_alphanumeric(),
        "alpha" => target_byte.is_ascii_alphabetic(),
        "blank" => target_byte == b' ' || target_byte == b'\t',
        "cntrl" => target_byte.is_ascii_control(),
        "digit" => target_byte.is_ascii_digit(),
        "graph" => target_byte.is_ascii() && !target_byte.is_ascii_control() && target_byte != b' ',
        "lower" => target_byte.is_ascii_lowercase(),
        "print" => target_byte.is_ascii() && !target_byte.is_ascii_control(),
        "punct" => target_byte.is_ascii_punctuation(),
        "space" => target_byte.is_ascii_whitespace(),
        "upper" => target_byte.is_ascii_uppercase(),
        "xdigit" => target_byte.is_ascii_hexdigit(),
        _ => false,
    }
}

fn find_bracket_end(pattern: &[WordChar]) -> Option<usize> {
    let mut i = 1;
    while i < pattern.len() {
        if pattern[i].is_wildcard(b']') {
            return Some(i);
        }
        if pattern[i].is_wildcard(b'[') && i + 1 < pattern.len() {
            let next_ch = pattern[i + 1].raw_byte();
            if (next_ch == b':' || next_ch == b'=' || next_ch == b'.')
                && !pattern[i + 1].is_quoted()
            {
                let mut found_closure = false;
                for j in (i + 2)..pattern.len() {
                    if j + 1 < pattern.len()
                        && !pattern[j].is_quoted()
                        && pattern[j].raw_byte() == next_ch
                        && pattern[j + 1].is_wildcard(b']')
                    {
                        i = j + 2;
                        found_closure = true;
                        break;
                    }
                }
                if found_closure {
                    continue;
                }
            }
        }
        i += 1;
    }
    None
}

/// Checks whether a slice of classified `WordChar` items matches the target segment text.
///
/// Quoted characters inside `pattern` only match their exact literal byte and are not evaluated
/// as wildcards.
pub fn match_segment_glob(pattern: &[WordChar], text: &BStr) -> bool {
    match_segment_helper(pattern, text.as_bytes())
}

fn eval_bracket_set(pattern: &[WordChar], target_byte: u8) -> Option<(bool, usize)> {
    let end = find_bracket_end(pattern)?;
    let set_content = &pattern[1..end];
    let mut negate = false;
    let mut match_chars = set_content;
    if !set_content.is_empty() {
        if set_content[0].is_wildcard(b'!') {
            negate = true;
            match_chars = &set_content[1..];
        }
    }
    let mut found = false;
    let mut i = 0;
    while i < match_chars.len() {
        if i + 1 < match_chars.len() {
            if match_chars[i].is_wildcard(b'[') && match_chars[i + 1].is_wildcard(b':') {
                let mut j_opt = None;
                for j in (i + 2)..match_chars.len() {
                    if j + 1 < match_chars.len() {
                        if match_chars[j].is_wildcard(b':') && match_chars[j + 1].is_wildcard(b']')
                        {
                            j_opt = Some(j);
                            break;
                        }
                    }
                }
                if let Some(j) = j_opt {
                    let mut class_name = String::new();
                    for k in (i + 2)..j {
                        class_name.push(match_chars[k].raw_byte() as char);
                    }
                    if match_char_class(target_byte, &class_name) {
                        found = true;
                    }
                    i = j + 2;
                    continue;
                }
            }
        }

        if i + 2 < match_chars.len() {
            if match_chars[i + 1].is_wildcard(b'-')
                && !match_chars[i].is_quoted()
                && !match_chars[i + 2].is_quoted()
            {
                let start = match_chars[i].raw_byte();
                let end = match_chars[i + 2].raw_byte();
                if target_byte >= start && target_byte <= end {
                    found = true;
                }
                i += 3;
                continue;
            }
        }

        if target_byte == match_chars[i].raw_byte() {
            found = true;
        }
        i += 1;
    }
    let matched = if negate { !found } else { found };
    Some((matched, end))
}

fn match_segment_helper(pattern: &[WordChar], target: &[u8]) -> bool {
    if pattern.is_empty() {
        return target.is_empty();
    }
    if pattern[0].is_wildcard(b'*') {
        if match_segment_helper(&pattern[1..], target) {
            return true;
        }
        if !target.is_empty() && match_segment_helper(pattern, &target[1..]) {
            return true;
        }
        false
    } else if pattern[0].is_wildcard(b'?') {
        if target.is_empty() {
            return false;
        }
        match_segment_helper(&pattern[1..], &target[1..])
    } else if pattern[0].is_wildcard(b'[') {
        if target.is_empty() {
            return false;
        }
        if let Some((matched, end)) = eval_bracket_set(pattern, target[0]) {
            if matched { match_segment_helper(&pattern[end + 1..], &target[1..]) } else { false }
        } else {
            if target[0] != b'[' {
                return false;
            }
            match_segment_helper(&pattern[1..], &target[1..])
        }
    } else {
        if target.is_empty() || target[0] != pattern[0].raw_byte() {
            return false;
        }
        match_segment_helper(&pattern[1..], &target[1..])
    }
}

/// Performs filesystem glob expansion on a word composed of `WordChar` elements.
///
/// Traverses the filesystem matching wildcard segments against directory contents.
/// Results are sorted lexicographically. If no filesystem matches are found, or if `word`
/// contains no unquoted wildcards, returns a single element containing the literal unquoted text.
///
/// Note: This implementation performs a purely string-based directory traversal and does
/// not evaluate or follow symbolic links. This shell is designed specifically for Fuchsia
/// native filesystems, which do not support symbolic links.
pub fn expand_glob(word: &[WordChar]) -> Vec<BString> {
    let has_wildcard = word.iter().any(|c| c.is_glob_wildcard());
    if !has_wildcard {
        return vec![word_chars_to_bstring(word)];
    }

    let mut segments = Vec::new();
    let mut current_seg = Vec::new();
    for c in word {
        let ch = c.raw_byte();
        if ch == b'/' {
            segments.push(std::mem::take(&mut current_seg));
        } else {
            current_seg.push(c.clone());
        }
    }
    segments.push(current_seg);

    let is_absolute = word.first().map_or(false, |c| c.raw_byte() == b'/');

    let mut results = Vec::new();

    fn traverse(
        base_dir: &std::path::Path,
        current_path: Option<BString>,
        segments: &[Vec<WordChar>],
        results: &mut Vec<BString>,
    ) {
        if segments.is_empty() {
            if let Some(path) = current_path {
                results.push(path);
            }
            return;
        }

        let seg = &segments[0];
        let has_wildcard = seg.iter().any(|c| c.is_glob_wildcard());

        if !has_wildcard {
            let seg_bytes = word_chars_to_bstring(seg);

            if seg_bytes.is_empty() {
                traverse(base_dir, current_path, &segments[1..], results);
            } else {
                let seg_bstr = seg_bytes.as_bstr();
                if let Ok(path_seg) = seg_bstr.to_path() {
                    let next_dir = base_dir.join(path_seg);
                    if next_dir.exists() {
                        let next_path = match &current_path {
                            Some(p) => {
                                if p == "/" {
                                    let mut joined = Vec::from(p.clone());
                                    joined.extend_from_slice(&seg_bytes);
                                    BString::from(joined)
                                } else {
                                    let mut joined = Vec::from(p.clone());
                                    joined.push(b'/');
                                    joined.extend_from_slice(&seg_bytes);
                                    BString::from(joined)
                                }
                            }
                            None => seg_bytes,
                        };
                        traverse(&next_dir, Some(next_path), &segments[1..], results);
                    }
                }
            }
        } else {
            if let Ok(entries) = std::fs::read_dir(base_dir) {
                let mut entry_names = Vec::new();
                for entry in entries {
                    if let Ok(entry) = entry {
                        let name_bytes = entry.file_name().as_bytes().to_vec();
                        entry_names.push(BString::from(name_bytes));
                    }
                }
                quick_sort(&mut entry_names, &|a, b| a.cmp(b));

                for name_bstr in entry_names {
                    let is_dotfile = name_bstr.starts_with(b".");
                    let starts_with_dot_pattern =
                        seg.first().map_or(false, |c| c.raw_byte() == b'.');
                    if is_dotfile && !starts_with_dot_pattern {
                        continue;
                    }

                    if match_segment_glob(seg, name_bstr.as_ref()) {
                        if let Ok(path_seg) = name_bstr.to_path() {
                            let next_dir = base_dir.join(path_seg);
                            let next_path = match &current_path {
                                Some(p) => {
                                    if p == "/" {
                                        let mut joined = Vec::from(p.clone());
                                        joined.extend_from_slice(&name_bstr);
                                        BString::from(joined)
                                    } else {
                                        let mut joined = Vec::from(p.clone());
                                        joined.push(b'/');
                                        joined.extend_from_slice(&name_bstr);
                                        BString::from(joined)
                                    }
                                }
                                None => name_bstr.clone(),
                            };
                            traverse(&next_dir, Some(next_path), &segments[1..], results);
                        }
                    }
                }
            }
        }
    }

    if is_absolute {
        traverse(std::path::Path::new("/"), Some(BString::from("/")), &segments[1..], &mut results);
    } else {
        traverse(std::path::Path::new("."), None, &segments, &mut results);
    }

    if results.is_empty() { vec![word_chars_to_bstring(word)] } else { results }
}
