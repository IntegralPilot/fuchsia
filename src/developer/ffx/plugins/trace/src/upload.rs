// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use ffx_config::EnvironmentContext;
use gcs::client::Client;
use gcs::error::GcsError;
use pbms::{AuthFlowChoice, handle_new_access_token};
use sha2::{Digest, Sha256};
use std::io::{IsTerminal, stderr, stdin, stdout};
use std::path::Path;

pub(crate) const CONFIG_GCS_BUCKET: &str = "trace.gcs_bucket";
pub(crate) const CONFIG_VIEWER_URL: &str = "trace.viewer_url";
pub(crate) const DEFAULT_GCS_BUCKET: &str = "fuchsia-trace-viewer-traces";
pub(crate) const DEFAULT_VIEWER_URL: &str = "https://fuchsia-trace-viewer.corp.goog";

pub(crate) fn resolve_bucket(cli_bucket: Option<&str>, context: &EnvironmentContext) -> String {
    if let Some(bucket) = cli_bucket {
        let trimmed = bucket.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(bucket) = context.get::<String, _>(CONFIG_GCS_BUCKET) {
        let trimmed = bucket.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DEFAULT_GCS_BUCKET.to_string()
}

pub(crate) fn resolve_viewer_url(context: &EnvironmentContext) -> String {
    if let Ok(url) = context.get::<String, _>(CONFIG_VIEWER_URL) {
        let trimmed = url.trim().trim_end_matches("/");
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DEFAULT_VIEWER_URL.to_string()
}

pub(crate) async fn compute_sha256_file(path: &Path) -> Result<String> {
    use futures::AsyncReadExt as _;
    let mut file = async_fs::File::open(path).await.with_context(|| {
        format!("Failed to open trace file for SHA-256 calculation: {:?}", path)
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        match file.read(&mut buffer).await {
            Ok(0) => break,
            Ok(bytes_read) => hasher.update(&buffer[..bytes_read]),
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("Failed to read trace file for SHA-256 calculation: {:?}", path)
                });
            }
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn extract_hash_from_key(key: &str) -> &str {
    let mut k = key.trim_start_matches('/');
    if k.len() >= 7 && k.is_char_boundary(7) && k[..7].eq_ignore_ascii_case("sha256/") {
        k = &k[7..];
    }
    k.split_once('.').map_or(k, |(stem, _)| stem)
}

pub(crate) fn calculate_shortest_unique_prefix(
    full_hash: &str,
    existing_keys: &[String],
) -> String {
    let full_hash_lower = full_hash.to_ascii_lowercase();
    let mut min_len = 8.min(full_hash_lower.len());
    let other_hashes: Vec<String> = existing_keys
        .iter()
        .filter_map(|key| {
            let hash = extract_hash_from_key(key).to_ascii_lowercase();
            if hash != full_hash_lower { Some(hash) } else { None }
        })
        .collect();

    while min_len < full_hash_lower.len() {
        let prefix = &full_hash_lower[..min_len];
        let collision = other_hashes.iter().any(|other| other.starts_with(prefix));
        if !collision {
            break;
        }
        min_len += 1;
    }

    full_hash_lower[..min_len].to_string()
}

pub(crate) fn format_trace_viewer_url(prefix: &str, viewer_base: &str) -> String {
    format!("{}/trace/{}", viewer_base.trim_end_matches("/"), prefix)
}

fn try_adc_access_token() -> Result<String> {
    let output = std::process::Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .context("Executing gcloud auth application-default print-access-token")?;
    if !output.status.success() {
        anyhow::bail!(
            "gcloud auth application-default print-access-token failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn refresh_access_token(client: &Client) -> Result<()> {
    // 1. Try Application Default Credentials (ADC) token first. On corp/workstation
    // machines, standard gcloud tokens may fail with 401 on GCS APIs while ADC succeeds.
    // In the ADC success case, stdin/terminal is not required.
    let access_token = match try_adc_access_token() {
        Ok(token) => token,
        Err(_) => {
            if cfg!(test) || !stdin().is_terminal() {
                anyhow::bail!(
                    "Cannot refresh GCS access token in a non-interactive or test session."
                );
            }
            let mut input = stdin();
            let mut output = stdout();
            let mut error_output = stderr();
            let ui = structured_ui::TextUi::new(&mut input, &mut output, &mut error_output);

            // 2. Try standard gcloud auth token.
            match handle_new_access_token(&AuthFlowChoice::Gcloud, &ui).await {
                Ok(token) => token,
                Err(_) => {
                    // 3. Fall back to Default PKCE flow.
                    handle_new_access_token(&AuthFlowChoice::Default, &ui)
                        .await
                        .context("Getting new access token")?
                }
            }
        }
    };
    client.set_access_token(access_token).await;
    Ok(())
}

async fn list_with_auth(client: &Client, bucket: &str, prefix: &str) -> Result<Vec<String>> {
    let mut auth_attempted = false;
    loop {
        match client.list(bucket, prefix).await {
            Ok(keys) => return Ok(keys),
            Err(e) => match e.downcast_ref::<GcsError>() {
                Some(GcsError::NeedNewAccessToken) if !auth_attempted => {
                    auth_attempted = true;
                    refresh_access_token(client).await.context(
                        "GCS authentication failed. Please ensure you are authenticated by running:
  gcloud auth application-default login
  gcloud auth login",
                    )?;
                }
                Some(GcsError::NeedNewAccessToken) => {
                    anyhow::bail!(
                        "GCS request was rejected as unauthorized (HTTP 401/403). To authenticate access to GCS bucket '{}', please run:
  gcloud auth application-default login
  gcloud auth login",
                        bucket
                    );
                }
                Some(GcsError::NotFound(_, _)) => return Ok(vec![]),
                _ => {
                    return Err(e).with_context(|| {
                        format!("Failed to list objects in GCS bucket '{}'", bucket)
                    });
                }
            },
        }
    }
}

async fn upload_with_auth(
    client: &Client,
    bucket: &str,
    object_name: &str,
    file_path: &Path,
) -> Result<()> {
    let path_buf = file_path.to_path_buf();
    let mut auth_attempted = false;
    loop {
        match client.upload(bucket, object_name, &path_buf).await {
            Ok(_) => return Ok(()),
            Err(e) => match e.downcast_ref::<GcsError>() {
                Some(GcsError::NeedNewAccessToken) if !auth_attempted => {
                    auth_attempted = true;
                    refresh_access_token(client).await.context(
                        "GCS authentication failed. Please ensure you are authenticated by running:
  gcloud auth application-default login
  gcloud auth login",
                    )?;
                }
                Some(GcsError::NeedNewAccessToken) => {
                    anyhow::bail!(
                        "GCS upload was rejected as unauthorized (HTTP 401/403). To authenticate access to GCS bucket '{}', please run:
  gcloud auth application-default login
  gcloud auth login",
                        bucket
                    );
                }
                _ => {
                    return Err(e).with_context(|| {
                        format!(
                            "Failed to upload object '{}' to GCS bucket '{}'",
                            object_name, bucket
                        )
                    });
                }
            },
        }
    }
}

pub(crate) async fn upload_trace(
    file_path: &Path,
    bucket: &str,
    viewer_base: &str,
) -> Result<String> {
    let metadata = std::fs::metadata(file_path)
        .with_context(|| format!("Failed to read metadata for trace file: {:?}", file_path))?;
    if metadata.len() == 0 {
        anyhow::bail!("cannot upload zero length traces");
    }
    let full_hash = compute_sha256_file(file_path).await?;
    let prefix_8 = &full_hash[..8.min(full_hash.len())];
    let query_prefix = format!("sha256/{}", prefix_8);

    let client = Client::initial().context("Initializing GCS client")?;
    if let Err(e) = refresh_access_token(&client).await {
        log::debug!("Initial GCS token refresh failed: {e:#}");
    }
    let listed_keys = list_with_auth(&client, bucket, &query_prefix).await?;

    let exists =
        listed_keys.iter().any(|k| extract_hash_from_key(k).eq_ignore_ascii_case(&full_hash));

    if !exists {
        let object_name = format!("sha256/{}.fxt", full_hash);
        upload_with_auth(&client, bucket, &object_name, file_path)
            .await
            .with_context(|| format!("Uploading {} to GCS bucket {}", object_name, bucket))?;
    }

    let shortest_prefix = calculate_shortest_unique_prefix(&full_hash, &listed_keys);
    Ok(format_trace_viewer_url(&shortest_prefix, viewer_base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    #[fuchsia::test]
    async fn test_compute_sha256() {
        let known_input = b"hello world";
        let expected_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(hex::encode(Sha256::digest(known_input)), expected_hash);

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(known_input).unwrap();
        let file_hash = compute_sha256_file(temp_file.path()).await.unwrap();
        assert_eq!(file_hash, expected_hash);
    }

    #[fuchsia::test]
    async fn test_upload_trace_zero_length() {
        let temp_file = NamedTempFile::new().unwrap();
        let res = upload_trace(temp_file.path(), "test-bucket", "https://example.com").await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert_eq!(err_msg, "cannot upload zero length traces");
    }

    #[fuchsia::test]
    async fn test_compute_sha256_large_stream() {
        let chunk = b"a".repeat(1024);
        let mut temp_file = NamedTempFile::new().unwrap();
        for _ in 0..10 {
            temp_file.write_all(&chunk).unwrap();
        }
        let file_hash = compute_sha256_file(temp_file.path()).await.unwrap();
        assert_eq!(file_hash.len(), 64);
    }

    #[test]
    fn test_extract_hash_from_key() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(extract_hash_from_key(&format!("sha256/{}.fxt", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("/sha256/{}.fxt", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("{}.fxt", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(full_hash), full_hash);

        // Arbitrary extensions (.perfetto-trace, .pb, .trace, .json, .tar.gz, etc.)
        assert_eq!(
            extract_hash_from_key(&format!("sha256/{}.perfetto-trace", full_hash)),
            full_hash
        );
        assert_eq!(
            extract_hash_from_key(&format!("/sha256/{}.perfetto-trace", full_hash)),
            full_hash
        );
        assert_eq!(extract_hash_from_key(&format!("{}.perfetto-trace", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("sha256/{}.pb", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("sha256/{}.trace", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("sha256/{}.json", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("sha256/{}.tar.gz", full_hash)), full_hash);

        // Case-insensitive prefix and suffix stripping
        assert_eq!(extract_hash_from_key(&format!("SHA256/{}.FXT", full_hash)), full_hash);
        assert_eq!(extract_hash_from_key(&format!("/Sha256/{}.Fxt", full_hash)), full_hash);
        assert_eq!(
            extract_hash_from_key(&format!("SHA256/{}.PERFETTO-TRACE", full_hash)),
            full_hash
        );
        assert_eq!(
            extract_hash_from_key(&format!("/Sha256/{}.Perfetto-Trace", full_hash)),
            full_hash
        );
        assert_eq!(extract_hash_from_key(&format!("SHA256/{}.PB", full_hash)), full_hash);

        // UTF-8 safety
        assert_eq!(extract_hash_from_key("🚀/test.fxt"), "🚀/test");
        assert_eq!(extract_hash_from_key("🚀/test.perfetto-trace"), "🚀/test");
    }

    #[test]
    fn test_shortest_unique_prefix_perfetto_trace_extension() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let same_key = vec![format!("sha256/{}.perfetto-trace", full_hash)];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &same_key), "b94d27b9");

        let colliding = vec![
            "sha256/b94d27b900000000000000000000000000000000000000000000000000000000.perfetto-trace"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding), "b94d27b99");
    }

    #[test]
    fn test_shortest_unique_prefix_no_collisions() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        // No existing keys
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &[]), "b94d27b9");
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &[]).len(), 8);

        // Existing key is the exact same hash (deduplication case, not a collision)
        let same_key = vec![format!("sha256/{}.fxt", full_hash)];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &same_key), "b94d27b9");

        // Existing keys with completely different prefixes
        let other_keys = vec![
            "sha256/11111111934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9.fxt"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &other_keys), "b94d27b9");
    }

    #[test]
    fn test_shortest_unique_prefix_leading_slashes() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        // Same hash with leading slash -> ignored for collision
        let same_key_slash = vec![format!("/sha256/{}.fxt", full_hash)];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &same_key_slash), "b94d27b9");

        // Colliding hash with leading slash -> must be detected and extend prefix length
        let colliding_slash = vec![
            "/sha256/b94d27b900000000000000000000000000000000000000000000000000000000.fxt"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_slash), "b94d27b99");
    }

    #[test]
    fn test_shortest_unique_prefix_with_collisions() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        // Collision on the first 8 characters -> should extend to 9 characters ("b94d27b99")
        let colliding_8 = vec![
            "sha256/b94d27b900000000000000000000000000000000000000000000000000000000.fxt"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_8), "b94d27b99");
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_8).len(), 9);

        // Collision on 10 characters -> should extend to 11 characters
        let colliding_10 = vec![
            "sha256/b94d27b993000000000000000000000000000000000000000000000000000000.fxt"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_10), "b94d27b9934");
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_10).len(), 11);
    }

    #[test]
    fn test_shortest_unique_prefix_multiple_collisions() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        let multiple = vec![
            "sha256/b94d27b900000000000000000000000000000000000000000000000000000000.fxt"
                .to_string(),
            "/sha256/b94d27b993000000000000000000000000000000000000000000000000000000.fxt"
                .to_string(),
            format!("sha256/{}.fxt", full_hash), // exact match should be ignored
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &multiple), "b94d27b9934");
    }

    #[test]
    fn test_shortest_unique_prefix_case_insensitivity() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        // Uppercase exact match -> deduplication case, ignored for collision
        let same_key_upper = vec![format!("sha256/{}.fxt", full_hash.to_ascii_uppercase())];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &same_key_upper), "b94d27b9");

        // Uppercase colliding hash -> must be detected and extend prefix length
        let colliding_upper = vec![
            "sha256/B94D27B900000000000000000000000000000000000000000000000000000000.fxt"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_upper), "b94d27b99");
    }

    #[test]
    fn test_shortest_unique_prefix_case_insensitive_prefix_and_extension() {
        let full_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        // Existing duplicate with uppercase prefix/extension -> deduplication case
        let duplicate_upper_ext = vec![format!("SHA256/{}.FXT", full_hash.to_ascii_uppercase())];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &duplicate_upper_ext), "b94d27b9");

        // Colliding key with uppercase prefix/extension -> must be detected
        let colliding_upper_ext = vec![
            "/SHA256/B94D27B900000000000000000000000000000000000000000000000000000000.FXT"
                .to_string(),
        ];
        assert_eq!(calculate_shortest_unique_prefix(full_hash, &colliding_upper_ext), "b94d27b99");
    }

    #[fuchsia::test]
    async fn test_compute_sha256_empty_file() {
        let temp_file = NamedTempFile::new().unwrap();
        let file_hash = compute_sha256_file(temp_file.path()).await.unwrap();
        let expected_empty_hash =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(file_hash, expected_empty_hash);
    }

    #[test]
    fn test_format_trace_viewer_url() {
        assert_eq!(
            format_trace_viewer_url("b94d27b9", "https://fuchsia-trace-viewer.corp.goog"),
            "https://fuchsia-trace-viewer.corp.goog/trace/b94d27b9"
        );
        assert_eq!(
            format_trace_viewer_url("b94d27b9", "https://custom-viewer.corp.goog/"),
            "https://custom-viewer.corp.goog/trace/b94d27b9"
        );
    }

    #[test]
    fn test_resolve_viewer_url() {
        let env = ffx_config::test_init().unwrap();
        let context = &env.context;
        assert_eq!(resolve_viewer_url(context), DEFAULT_VIEWER_URL);
    }

    #[test]
    fn test_resolve_viewer_url_with_config() {
        let env = ffx_config::test_env()
            .user_config("trace.viewer_url", "https://custom-viewer.corp.goog/")
            .build()
            .unwrap();
        let context = &env.context;
        assert_eq!(resolve_viewer_url(context), "https://custom-viewer.corp.goog");
    }

    #[test]
    fn test_resolve_bucket() {
        let env = ffx_config::test_init().unwrap();
        let context = &env.context;

        // 1. CLI flag override
        assert_eq!(resolve_bucket(Some("cli-bucket"), context), "cli-bucket");
        assert_eq!(resolve_bucket(Some(" cli-bucket "), context), "cli-bucket");

        // 2. Empty string CLI flag falls back to default
        assert_eq!(resolve_bucket(Some(""), context), DEFAULT_GCS_BUCKET);
        assert_eq!(resolve_bucket(Some("   "), context), DEFAULT_GCS_BUCKET);

        // 3. Default fallback when no config is set
        assert_eq!(resolve_bucket(None, context), DEFAULT_GCS_BUCKET);
    }

    #[test]
    fn test_resolve_bucket_with_config() {
        let env = ffx_config::test_env()
            .user_config("trace.gcs_bucket", "config-bucket")
            .build()
            .unwrap();
        let context = &env.context;

        // Config should be used when CLI option is None
        assert_eq!(resolve_bucket(None, context), "config-bucket");

        // CLI flag should still override config
        assert_eq!(resolve_bucket(Some("cli-override"), context), "cli-override");

        // Empty CLI flag should fall back to config
        assert_eq!(resolve_bucket(Some(""), context), "config-bucket");
    }
}
