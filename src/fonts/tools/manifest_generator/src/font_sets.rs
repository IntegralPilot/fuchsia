// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Deserialization for `.local_fonts.json` files.
//!
//! Generated per target product using the `generated_file` GN rule (see
//! "//src/fonts/build/fonts.gni").

use crate::serde_ext::{self, LoadError};
use serde::Deserialize;
use std::collections::btree_map::Iter as BTreeMapIter;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Describes which set a font belongs to, local or downloadable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum FontSet {
    /// Font files in the local set. Bundled directly in the font server's `/config/data`.
    Local,
    /// Available to download as a Fuchsia package (`fuchsia-pkg://fuchsia.com/font-package-...`).
    Download,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FontSets {
    map: BTreeMap<String, FontSet>,
}

impl FontSets {
    pub fn load_from_local_paths<T: AsRef<Path>>(local_path: T) -> Result<Self, LoadError> {
        let local_file_names: BTreeSet<String> = serde_ext::load_from_path(local_path)?;

        let mut map = BTreeMap::new();
        for file_name in local_file_names {
            map.insert(file_name, FontSet::Local);
        }

        Ok(Self::new(map))
    }

    /// Creates a new `FontSets` collection from a map of font file names to [`FontSet`] values,
    /// as generated in [`FontSets::load_from_local_paths`].
    ///
    /// _Visible for tests only._
    pub(crate) fn new(map: BTreeMap<String, FontSet>) -> Self {
        Self { map }
    }

    pub fn get_font_set(&self, file_name: &str) -> Option<&FontSet> {
        self.map.get(file_name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &FontSet)> {
        self.into_iter()
    }
}

impl<'a> IntoIterator for &'a FontSets {
    type Item = (&'a String, &'a FontSet);
    type IntoIter = BTreeMapIter<'a, String, FontSet>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_local_paths() -> Result<(), Error> {
        let local_fonts_contents = json!(["a.ttf", "b.ttf", "c.ttf"]).to_string();
        let mut local_fonts_file = NamedTempFile::new()?;
        local_fonts_file.write_all(local_fonts_contents.as_bytes())?;

        let font_sets = FontSets::load_from_local_paths(local_fonts_file.path())?;

        assert_eq!(font_sets.get_font_set("a.ttf"), Some(&FontSet::Local));
        assert_eq!(font_sets.get_font_set("b.ttf"), Some(&FontSet::Local));
        assert_eq!(font_sets.get_font_set("c.ttf"), Some(&FontSet::Local));
        assert_eq!(font_sets.get_font_set("404.ttf"), None);

        Ok(())
    }
}
