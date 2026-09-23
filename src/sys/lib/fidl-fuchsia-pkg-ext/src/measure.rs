// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg as fidl;

/// FIDL types that can have their in-line and out-of-line message byte payload size measured.
pub trait Measurable {
    /// Determine the message byte count for this instance.
    fn measure(&self) -> usize;
}

impl Measurable for fidl::BlobId {
    fn measure(&self) -> usize {
        measure_fuchsia_pkg::Measurable::measure(self).num_bytes
    }
}

impl Measurable for fidl::BlobInfo {
    fn measure(&self) -> usize {
        measure_fuchsia_pkg::Measurable::measure(self).num_bytes
    }
}

impl Measurable for fidl::PackageIndexEntry {
    fn measure(&self) -> usize {
        measure_fuchsia_pkg::Measurable::measure(self).num_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Truncates `s` to be at most `max_len` bytes.
    fn truncate_str(s: &str, max_len: usize) -> &str {
        let index = s.floor_char_boundary(max_len);
        &s[..index]
    }

    prop_compose! {
        fn arb_package_index_entry()(
            url in ".{0,2048}",
            blob_id: crate::BlobId,
        ) -> fidl::PackageIndexEntry {
            // The regex accepts up to 2048 Unicode code points, but the max
            // length from FIDL is in terms of bytes, not code points.
            let url = truncate_str(&url, 2048).to_owned();
            fidl::PackageIndexEntry {
                package_url: fidl::PackageUrl { url, },
                meta_far_blob_id: blob_id.into(),
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig{
            // Disable persistence to avoid the warning for not running in the
            // source code directory (since we're running on a Fuchsia target)
            failure_persistence: None,
            .. ProptestConfig::default()
        })]

        #[test]
        fn blob_id_size_is_as_bytes_size(item: crate::BlobId) {
            let item: fidl::BlobId = item.into();

            let expected = std::mem::size_of_val(&item);
            let actual = item.measure();
            prop_assert_eq!(expected, actual);
        }

        #[test]
        fn blob_info_size_is_as_bytes_size(item: crate::BlobInfo) {
            let item: fidl::BlobInfo = item.into();

            let expected = std::mem::size_of_val(&item);
            let actual = item.measure();
            prop_assert_eq!(expected, actual);
        }

        #[test]
        fn package_index_entry_size_is_fidl_encoded_size(
            item in arb_package_index_entry()
        ) {
            let actual = item.measure();
            let (bytes, _) =
                ::fidl::standalone_encode_value(&item).unwrap();
            let expected = bytes.len();
            prop_assert_eq!(expected, actual);
        }
    }
}
