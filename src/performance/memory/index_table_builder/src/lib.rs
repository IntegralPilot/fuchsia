// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cmp::Eq;
use std::collections::HashMap;
use std::hash::Hash;

/// Deduplicate values, returning the index to the unique occurrence in an array.
pub struct IndexTableBuilder<T>
where
    T: Eq + Hash + Default + 'static,
{
    index: HashMap<T, usize>,
}

impl<T> Default for IndexTableBuilder<T>
where
    T: Eq + Hash + Default + 'static,
{
    fn default() -> IndexTableBuilder<T> {
        IndexTableBuilder { index: HashMap::new() }
    }
}

impl<T> IndexTableBuilder<T>
where
    T: Eq + Hash + Default + 'static,
{
    /// Inserts a value, if not present yet, and returns its index.
    pub fn intern(&mut self, value: impl Into<T>) -> usize {
        let next_index = self.index.len();
        *self.index.entry(value.into()).or_insert(next_index)
    }

    /// Consumes this builder and returns all the values that were interned.
    pub fn build(self) -> Vec<T> {
        let mut table = Vec::default();
        table.resize_with(self.index.len(), Default::default);
        self.index.into_iter().for_each(|(element, index)| {
            table[index] = element;
        });
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::zip;

    #[test]
    fn test_starts_with_empty_string() {
        let result = IndexTableBuilder::default().build();
        assert_eq!(vec![], result);
    }

    #[test]
    fn test_mostly_empty() {
        let b = IndexTableBuilder::default();
        assert_eq!(0, b.intern("singleton"));
        assert_eq!(vec!["singleton"], b.build());
    }

    /// Ensure the table can intern unique words.
    #[test]
    fn test_interns_unique_words() {
        let mut b = IndexTableBuilder::default();
        let words = vec!["Those", "are", "unique", "words"];
        let indices: Vec<i64> = words.iter().map(|w| b.intern(w)).collect();
        let result = b.build();

        for (index, word) in zip(indices, words.iter()) {
            assert_eq!(*word, result[index as usize]);
        }
    }

    /// Ensure the table can intern repeated words, and reuses
    /// the same entries.
    #[test]
    fn test_interns_repeated_words() {
        let mut b = IndexTableBuilder::default();
        let words = vec!["word", "word", "word", "word"];
        let indices: Vec<i64> = words.iter().map(|w| b.intern(w)).collect();
        let result = b.build();

        let repeated = indices[0];
        for (index, word) in zip(indices, words.iter()) {
            assert_eq!(*word, result[index as usize]);
            assert_eq!(repeated, index);
        }
    }
}
