// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use component_id_index::Index;
use fidl::persist;
use fidl_fuchsia_component_internal::ComponentIdIndex;

/// A builder that constructs component id indices.
#[derive(Default)]
pub struct ComponentIdIndexBuilder {
    indices: Vec<Utf8PathBuf>,
}

impl ComponentIdIndexBuilder {
    /// Add a product-provided index.
    pub fn index(&mut self, index: impl AsRef<Utf8Path>) -> &mut Self {
        self.indices.push(index.as_ref().into());
        self
    }

    /// Build the final index by merging all the indices from the product and
    /// optionally the core component id index.
    pub fn build(self, outdir: impl AsRef<Utf8Path>) -> Result<(Index, Utf8PathBuf)> {
        let merged_index =
            Index::merged_from_json5_files(&self.indices).context("merging index files")?;
        let index = merged_index.clone();
        let merged_index_fidl: ComponentIdIndex =
            merged_index.try_into().context("converting index into fidl")?;
        let bytes = persist(&merged_index_fidl).context("Could not fidl-encode index")?;

        let path = outdir.as_ref().join("component_id_index.fidlbin");
        std::fs::write(&path, bytes)
            .context("Could not write merged FIDL-encoded index to file")?;
        Ok((index, path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_id_index::{IndexEntry, InstanceId};
    use moniker::Moniker;
    use pretty_assertions::assert_eq;
    use std::io::Write;

    fn gen_index(start: u32, end: u32) -> Index {
        let mut index = Index::default();
        for i in start..end {
            let moniker = Moniker::parse_str(&format!("/a/b/c/{i}")).unwrap();
            let instance_id =
                format!("00000000000000000000000000000000000000000000000000000000000000{:02x}", i)
                    .parse::<InstanceId>()
                    .unwrap();
            index.insert(IndexEntry { moniker, instance_id, ignore_duplicate_id: false }).unwrap();
        }
        index
    }

    #[test]
    fn build_empty() {
        let outdir = tempfile::TempDir::new().unwrap();
        let outdir_path = Utf8PathBuf::from_path_buf(outdir.path().to_path_buf()).unwrap();
        let builder = ComponentIdIndexBuilder::default();
        let (_, path) = builder.build(&outdir_path).unwrap();
        assert_eq!(path, outdir_path.join("component_id_index.fidlbin"));
    }

    #[test]
    fn build_multiple_indices() {
        let mut tmp_input_index1 = tempfile::NamedTempFile::new().unwrap();
        let mut tmp_input_index2 = tempfile::NamedTempFile::new().unwrap();

        // write the first index file
        let index1 = gen_index(0, 2);
        let index1_path =
            Utf8PathBuf::from_path_buf(tmp_input_index1.path().to_path_buf()).unwrap();
        tmp_input_index1.write_all(serde_json5::to_string(&index1).unwrap().as_bytes()).unwrap();

        // write the second index file
        let index2 = gen_index(2, 4);
        let index2_path =
            Utf8PathBuf::from_path_buf(tmp_input_index2.path().to_path_buf()).unwrap();
        tmp_input_index2.write_all(serde_json5::to_string(&index2).unwrap().as_bytes()).unwrap();

        let outdir = tempfile::TempDir::new().unwrap();
        let outdir_path = Utf8PathBuf::from_path_buf(outdir.path().to_path_buf()).unwrap();
        let mut builder = ComponentIdIndexBuilder::default();
        builder.index(&index1_path);
        builder.index(&index2_path);
        let (_, path) = builder.build(&outdir_path).unwrap();
        assert_eq!(path, outdir_path.join("component_id_index.fidlbin"));
        let index = component_id_index::Index::from_fidl_file(&path).unwrap();

        for i in 0..4 {
            let mon = Moniker::parse_str(&format!("/a/b/c/{}", i)).unwrap();
            let instance = index.id_for_moniker(&mon).unwrap();
            let expected =
                format!("00000000000000000000000000000000000000000000000000000000000000{:02x}", i)
                    .parse::<InstanceId>()
                    .unwrap();
            assert!(index.contains_id(&expected));
            assert_eq!(&expected, instance);
        }
    }
}
