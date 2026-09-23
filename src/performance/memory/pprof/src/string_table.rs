// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use index_table_builder::IndexTableBuilder;

/// Helper data structure for string interning.
pub struct StringTableBuilder {
    table_builder: IndexTableBuilder<String>,
}

impl Default for StringTableBuilder {
    fn default() -> StringTableBuilder {
        let mut table_builder = IndexTableBuilder::default();
        table_builder.intern(""); // entry #0 must always be the empty string
        StringTableBuilder { table_builder }
    }
}

impl StringTableBuilder {
    /// Inserts a string, if not present yet, and returns its index.
    pub fn intern(&mut self, string: &str) -> i64 {
        self.table_builder.intern(string) as i64
    }

    /// Converts a build ID into a string and intern it.
    pub fn intern_build_id(&mut self, build_id: &[u8]) -> i64 {
        self.intern(&hex::encode(build_id))
    }

    /// Consumes this StringTableBuilder and returns all the strings that were interned.
    pub fn build(self) -> Vec<String> {
        self.table_builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::zip;

    /// `pprof` mandates that the string table starts with the empty
    /// string.
    #[test]
    fn test_starts_with_empty_string() {
        let result = StringTableBuilder::default().build();

        assert_eq!(vec![""], result);
    }

    /// Ensure that the string table knows to resolve the empty
    /// string.
    #[test]
    fn test_knows_empty_string() {
        let mut st = StringTableBuilder::default();

        assert_ne!(0, st.intern("word"));
        assert_eq!(0, st.intern(""));
    }

    /// Ensure the string table can intern unique words.
    #[test]
    fn test_interns_unique_words() {
        let mut st = StringTableBuilder::default();
        let words = vec!["Those", "are", "unique", "words"];
        let indices: Vec<i64> = words.iter().map(|w| st.intern(w)).collect();
        let result = st.build();

        for (index, word) in zip(indices, words.iter()) {
            assert_eq!(*word, result[index as usize]);
        }
    }

    /// Ensure the string table can intern repeated words, and reuses
    /// the same entries.
    #[test]
    fn test_interns_repeated_words() {
        let mut st = StringTableBuilder::default();
        let words = vec!["word", "word", "word", "word"];
        let indices: Vec<i64> = words.iter().map(|w| st.intern(w)).collect();
        let result = st.build();

        let repeated = indices[0];
        for (index, word) in zip(indices, words.iter()) {
            assert_eq!(*word, result[index as usize]);
            assert_eq!(repeated, index);
        }
    }

    #[test]
    fn test_intern_build_id() {
        let mut st = StringTableBuilder::default();

        let index = st.intern_build_id(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        let result = st.build();

        assert_eq!(result[index as usize], "0123456789abcdef");
    }
}
