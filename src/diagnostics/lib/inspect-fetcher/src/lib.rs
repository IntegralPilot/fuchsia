// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, bail};
use diagnostics_reader::{ArchiveReader, InspectArchiveReader, RetryConfig};
use log::warn;

// Selectors for Inspect data must start with this exact string.
const INSPECT_PREFIX: &str = "INSPECT:";

/// `InspectFetcher` fetches data from a list of selectors from ArchiveAccessor.
pub struct InspectFetcher {
    // If we have no selectors, we don't want to actually fetch anything.
    // (Fetching with no selectors fetches all Inspect data.)
    reader: Option<InspectArchiveReader>,
}

impl std::fmt::Debug for InspectFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectFetcher").field("reader", &"opaque-ArchiveReader").finish()
    }
}

impl InspectFetcher {
    /// Creates an InspectFetcher or returns an error. Note: If no selectors are given,
    /// fetch() will return "[]" instead of fetching all Inspect data.
    ///
    /// `service_path` should name a fuchsia.diagnostics.ArchiveAccessor service.
    /// `selectors` should be in Triage format, i.e. INSPECT:moniker:path:leaf.
    pub fn create(service_path: &str, selectors: Vec<String>) -> Result<InspectFetcher, Error> {
        if selectors.is_empty() {
            return Ok(InspectFetcher { reader: None });
        }
        let proxy = match fuchsia_component::client::connect_to_protocol_at_path::<
            fidl_fuchsia_diagnostics::ArchiveAccessorMarker,
        >(service_path)
        {
            Ok(proxy) => proxy,
            Err(e) => bail!("Failed to connect to Inspect reader: {}", e),
        };
        let mut reader = ArchiveReader::inspect();
        reader
            .with_archive(proxy)
            .retry(RetryConfig::never())
            .add_selectors(Self::process_selectors(selectors)?.into_iter());
        Ok(InspectFetcher { reader: Some(reader) })
    }

    /// Fetches the selectee Inspect data.
    /// Data is returned as a String in JSON format because that's what TriageLib needs.
    pub async fn fetch(&mut self) -> Result<String, Error> {
        match &self.reader {
            None => Ok("[]".to_string()),
            Some(reader) => {
                // TODO(https://fxbug.dev/42140879): Make TriageLib accept structured data
                Ok(reader.snapshot_raw::<serde_json::Value>().await?.to_string())
            }
        }
    }

    fn process_selectors(selectors: Vec<String>) -> Result<Vec<String>, Error> {
        Ok(selectors.into_iter().filter_map(remove_inspect_prefix).collect())
    }
}

/// Remove the "INSPECT:" prefix from selectors. Returns None if the Inspect
/// prefix is not found.
pub fn remove_inspect_prefix(mut s: String) -> Option<String> {
    if s.len() >= INSPECT_PREFIX.len() && s[..INSPECT_PREFIX.len()] == *INSPECT_PREFIX {
        s.replace_range(0..INSPECT_PREFIX.len(), "");
        Some(s)
    } else {
        warn!("All Inspect selectors should begin with 'INSPECT:' - '{}'", s);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_selector_acceptance() {
        let empty_vec = vec![];
        let ok_selectors =
            vec!["INSPECT:moniker:path:leaf".to_string(), "INSPECT:name:nodes:item".to_string()];
        let ok_processed = vec!["moniker:path:leaf".to_string(), "name:nodes:item".to_string()];

        let bad_selector = vec![
            "INSPECT:moniker:path:leaf".to_string(),
            "FOO:moniker:path:leaf".to_string(),
            "INSPECT:name:nodes:item".to_string(),
        ];

        assert_eq!(InspectFetcher::process_selectors(empty_vec).unwrap(), Vec::<String>::new());
        assert_eq!(InspectFetcher::process_selectors(ok_selectors).unwrap(), ok_processed);
        assert_eq!(InspectFetcher::process_selectors(bad_selector).unwrap(), ok_processed);
    }
}
