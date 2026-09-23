// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod blobfs;
pub mod far;
pub mod fvm;
pub mod package;
pub mod zbi;
pub mod zbi_bootfs;
pub mod zbi_cmdline;

#[cfg(test)]
mod tests {
    use crate::extract::fvm::FvmExtractController;
    use crate::extract::zbi::ZbiExtractController;
    use crate::extract::zbi_cmdline::ZbiExtractCmdlineController;
    use tempfile::tempdir;

    #[test]
    fn test_zbi_extractor_empty_zbi() {
        let input_dir = tempdir().unwrap();
        let input_path = input_dir.path().join("empty-zbi");
        let output_dir = tempdir().unwrap();
        let output_path = output_dir.path();
        let response = ZbiExtractController::extract(input_path, output_path.to_path_buf());
        assert_eq!(response.is_ok(), false);
    }

    #[test]
    fn test_zbi_cmdline_extractor_empty_zbi() {
        let input_dir = tempdir().unwrap();
        let input_path = input_dir.path().join("empty-zbi");
        let response = ZbiExtractCmdlineController::extract(input_path);
        assert_eq!(response.is_ok(), false);
    }

    #[test]
    fn test_fvm_extractor_empty_fvm() {
        let input_dir = tempdir().unwrap();
        let input_path = input_dir.path().join("empty-fvm");
        let output_dir = tempdir().unwrap();
        let output_path = output_dir.path();
        let response = FvmExtractController::extract(input_path, output_path.to_path_buf());
        assert_eq!(response.is_ok(), false);
    }
}
