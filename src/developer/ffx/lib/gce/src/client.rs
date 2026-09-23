// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::models::{
    FirewallAllowed, FirewallRule, Image, Instance, InstanceList, Operation, SerialPortOutput,
};
use anyhow::{Context, Result, bail};
use fuchsia_hyper::{HttpsClient, new_https_client};
use http_body_util::BodyExt;
use hyper::{Method, Request, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;
use url::Url;

pub type Body = http_body_util::Full<hyper::body::Bytes>;

/// Polling interval when waiting for GCE global or zonal operations to complete.
const OPERATION_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Chunk size (64 MiB) for GCS resumable uploads.
///
/// GCS requires non-final chunks to be multiples of 256 KiB. Each chunk requires a sequential
/// HTTP PUT + `308 Resume Incomplete` round-trip, so 64 MiB amortizes round-trip latency (~16
/// requests per GiB instead of 128 at 8 MiB) while keeping host memory usage and per-chunk
/// retry retransmission bounded to 64 MiB.
const GCS_UPLOAD_CHUNK_SIZE: usize = 64 * 1024 * 1024;

/// Maximum number of retry attempts for a failed chunk in a GCS resumable upload.
const GCS_UPLOAD_MAX_RETRIES: u32 = 3;

/// Delay between retry attempts for a failed GCS upload chunk.
const GCS_UPLOAD_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Google-internal Cloud Uberproxy IPv4 CIDR block used for SSH ingress to GCE VMs.
///
/// Note: External GCP environments or connections via Cloud Identity-Aware Proxy (IAP,
/// which uses `35.235.240.0/20`) require different source ranges; this could be made
/// configurable via `ffx config` in the future.
const SSH_INGRESS_SOURCE_RANGE: &str = "172.253.30.0/23";
const SSH_PORT: &str = "22";
const DEFAULT_FIREWALL_PRIORITY: u32 = 1000;

/// High-level client for interacting with Google Compute Engine (GCE) and
/// Google Cloud Storage (GCS) APIs.
#[derive(Debug)]
pub struct GceClient {
    http: HttpClient,
}

impl GceClient {
    pub fn new(access_token: String) -> Self {
        Self { http: HttpClient::new(access_token) }
    }

    pub async fn get_image(&self, project: &str, image_name: &str) -> Result<Image> {
        self.http.get_json(endpoints::image(project, image_name)?).await
    }

    pub async fn insert_image(&self, project: &str, image: &Image) -> Result<Operation> {
        self.http.post_json(endpoints::images(project)?, image).await
    }

    pub async fn delete_image(&self, project: &str, image_name: &str) -> Result<Operation> {
        self.http.delete_json(endpoints::image(project, image_name)?).await
    }

    pub async fn get_instance(
        &self,
        project: &str,
        zone: &str,
        instance_name: &str,
    ) -> Result<Instance> {
        self.http.get_json(endpoints::instance(project, zone, instance_name)?).await
    }

    pub async fn get_serial_port_output(
        &self,
        project: &str,
        zone: &str,
        instance_name: &str,
        port: u32,
        start: Option<i64>,
    ) -> Result<SerialPortOutput> {
        let url = endpoints::serial_port_output(project, zone, instance_name, port, start)?;
        self.http.get_json(url).await
    }

    pub async fn list_instances(&self, project: &str, zone: &str) -> Result<Vec<Instance>> {
        let list: InstanceList =
            self.http.get_json(endpoints::list_instances(project, zone)?).await?;
        Ok(list.items)
    }

    pub async fn insert_instance(
        &self,
        project: &str,
        zone: &str,
        instance: &Instance,
    ) -> Result<Operation> {
        self.http.post_json(endpoints::instances(project, zone)?, instance).await
    }

    pub async fn delete_instance(
        &self,
        project: &str,
        zone: &str,
        instance_name: &str,
    ) -> Result<Operation> {
        self.http.delete_json(endpoints::instance(project, zone, instance_name)?).await
    }

    pub async fn stop_instance(
        &self,
        project: &str,
        zone: &str,
        instance_name: &str,
    ) -> Result<Operation> {
        self.http.post_empty_json(endpoints::stop_instance(project, zone, instance_name)?).await
    }

    pub async fn start_instance(
        &self,
        project: &str,
        zone: &str,
        instance_name: &str,
    ) -> Result<Operation> {
        self.http.post_empty_json(endpoints::start_instance(project, zone, instance_name)?).await
    }

    pub async fn get_global_operation(&self, project: &str, op_name: &str) -> Result<Operation> {
        self.http.get_json(endpoints::global_operation(project, op_name)?).await
    }

    pub async fn get_zone_operation(
        &self,
        project: &str,
        zone: &str,
        op_name: &str,
    ) -> Result<Operation> {
        self.http.get_json(endpoints::zone_operation(project, zone, op_name)?).await
    }

    async fn wait_for_operation<F, Fut>(&self, mut get_op: F) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<Operation>>,
    {
        loop {
            let op = get_op().await?;

            if let Some(err) = op.error {
                let err_msg = err
                    .errors
                    .into_iter()
                    .map(|e| {
                        format!("{}: {}", e.code.unwrap_or_default(), e.message.unwrap_or_default())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("Operation failed: {}", err_msg);
            }

            if op.status.as_deref() == Some("DONE") {
                return Ok(());
            }

            fuchsia_async::Timer::new(OPERATION_POLL_INTERVAL).await;
        }
    }

    pub async fn wait_for_global_operation(&self, project: &str, op_name: &str) -> Result<()> {
        self.wait_for_operation(|| self.get_global_operation(project, op_name)).await
    }

    pub async fn wait_for_zone_operation(
        &self,
        project: &str,
        zone: &str,
        op_name: &str,
    ) -> Result<()> {
        self.wait_for_operation(|| self.get_zone_operation(project, zone, op_name)).await
    }

    pub async fn get_firewall_rule(&self, project: &str, rule_name: &str) -> Result<FirewallRule> {
        self.http.get_json(endpoints::firewall_rule(project, rule_name)?).await
    }

    pub async fn insert_firewall_rule(
        &self,
        project: &str,
        rule: &FirewallRule,
    ) -> Result<Operation> {
        self.http.post_json(endpoints::firewalls(project)?, rule).await
    }

    /// Best-effort helper to ensure a firewall ingress rule allowing SSH traffic exists for `network`.
    ///
    /// If checking or creating the firewall rule fails (for example, when the caller lacks
    /// `compute.firewalls.create` IAM permissions), a warning is logged and `Ok(())` is returned
    /// so VM connection attempts can still proceed if a rule was pre-provisioned.
    pub async fn ensure_ssh_firewall_rule(&self, project: &str, network: &str) -> Result<()> {
        let rule_name = ssh_firewall_rule_name(network);

        if self.get_firewall_rule(project, &rule_name).await.is_ok() {
            return Ok(());
        }

        let trimmed_network = network.trim_matches('/');
        let rule = FirewallRule {
            name: rule_name.clone(),
            network: Some(
                if trimmed_network.starts_with("global/networks/")
                    || trimmed_network.starts_with("projects/")
                    || trimmed_network.starts_with("https://")
                {
                    trimmed_network.to_string()
                } else {
                    let net_name =
                        if trimmed_network.is_empty() { "default" } else { trimmed_network };
                    format!("global/networks/{net_name}")
                },
            ),
            source_ranges: vec![SSH_INGRESS_SOURCE_RANGE.to_string()],
            allowed: vec![FirewallAllowed {
                ip_protocol: "tcp".to_string(),
                ports: vec![SSH_PORT.to_string()],
            }],
            direction: Some("INGRESS".to_string()),
            priority: Some(DEFAULT_FIREWALL_PRIORITY),
        };

        if let Err(e) = self.insert_firewall_rule(project, &rule).await {
            log::warn!("Failed to insert SSH firewall rule {rule_name}: {e:?}");
        }
        Ok(())
    }

    /// Ensures the specified GCS bucket exists by attempting to create it directly
    /// and treating `409 Conflict` (already exists) as success.
    pub async fn ensure_bucket(&self, project: &str, bucket: &str) -> Result<()> {
        let url = endpoints::create_bucket(project)?;
        let body = serde_json::to_vec(&serde_json::json!({ "name": bucket }))?;
        self.http.post_raw(url, "application/json", body, &[StatusCode::CONFLICT]).await
    }

    /// Uploads a local file to GCS using the GCS Resumable Upload protocol (`uploadType=resumable`),
    /// streaming the file in fixed-size chunks to avoid loading large disk images into memory at once.
    pub async fn upload_gcs_file(
        &self,
        bucket: &str,
        object_name: &str,
        file_path: &Path,
    ) -> Result<()> {
        let mut file = File::open(file_path)
            .with_context(|| format!("Failed to open file at {:?}", file_path))?;
        let total_size = file
            .metadata()
            .with_context(|| format!("Failed to read metadata for {:?}", file_path))?
            .len();

        let init_url = endpoints::upload_gcs_object(bucket, object_name)?;
        let session_url =
            self.http.start_resumable_upload(init_url, "application/gzip", total_size).await?;

        upload_reader_resumable(
            &mut file,
            total_size,
            GCS_UPLOAD_CHUNK_SIZE,
            GCS_UPLOAD_MAX_RETRIES,
            GCS_UPLOAD_RETRY_DELAY,
            |content_range, chunk, is_final| {
                let url = session_url.clone();
                async move {
                    self.http
                        .put_upload_chunk(url, "application/gzip", &content_range, chunk, is_final)
                        .await
                }
            },
        )
        .await
        .with_context(|| {
            format!("Failed to upload {:?} to gs://{}/{}", file_path, bucket, object_name)
        })
    }

    pub async fn delete_gcs_file(&self, bucket: &str, object_name: &str) -> Result<()> {
        let url = endpoints::delete_gcs_object(bucket, object_name)?;
        self.http.delete_raw(url, &[StatusCode::NOT_FOUND]).await
    }
}

fn ssh_firewall_rule_name(network: &str) -> String {
    let network_suffix = network
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    format!("allow-ssh-ingress-{network_suffix}")
}

/// Reads from `reader` until `buf` is completely filled or EOF is reached.
fn read_full_chunk(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total_read = 0;
    while total_read < buf.len() {
        match reader.read(&mut buf[total_read..]) {
            Ok(0) => break,
            Ok(n) => total_read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total_read)
}

/// Parses a GCS `Range` header (e.g., `bytes=0-8388607`) and returns the next byte offset (`8388608`).
fn parse_range_header(range_header: &str) -> Option<u64> {
    let range = range_header.trim().strip_prefix("bytes=")?;
    let (_, end_str) = range.split_once('-')?;
    let end_byte = end_str.parse::<u64>().ok()?;
    end_byte.checked_add(1)
}

/// Incrementally reads chunks from `reader` and uploads them via `put_chunk`, supporting
/// automatic retry on transient errors and resuming from server-reported `Range` offsets.
async fn upload_reader_resumable<R, F, Fut>(
    reader: &mut R,
    total_size: u64,
    chunk_size: usize,
    max_retries: u32,
    retry_delay: Duration,
    mut put_chunk: F,
) -> Result<()>
where
    R: Read + Seek,
    F: FnMut(String, Vec<u8>, bool) -> Fut,
    Fut: std::future::Future<Output = Result<Option<u64>>>,
{
    if total_size == 0 {
        let mut attempt = 0;
        loop {
            match put_chunk("bytes */0".to_string(), Vec::new(), true).await {
                Ok(_) => return Ok(()),
                Err(_) if attempt < max_retries => {
                    attempt += 1;
                    fuchsia_async::Timer::new(retry_delay).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    let buf_size = std::cmp::min(total_size, chunk_size as u64) as usize;
    let mut buffer = vec![0u8; buf_size];
    let mut offset: u64 = 0;

    while offset < total_size {
        reader.seek(SeekFrom::Start(offset)).context("Failed to seek upload file")?;
        let to_read = std::cmp::min(total_size - offset, buf_size as u64) as usize;
        let bytes_read = read_full_chunk(reader, &mut buffer[..to_read])
            .context("Failed to read chunk from upload file")?;
        if bytes_read == 0 {
            bail!("Unexpected EOF while reading upload file at offset {offset}/{total_size}");
        }

        let end = offset + (bytes_read as u64) - 1;
        let is_final = offset + (bytes_read as u64) == total_size;
        let content_range = format!("bytes {offset}-{end}/{total_size}");

        let mut attempt = 0;
        let next_offset = loop {
            let chunk_vec = buffer[..bytes_read].to_vec();
            match put_chunk(content_range.clone(), chunk_vec, is_final).await {
                Ok(reported_next) => break reported_next,
                Err(_) if attempt < max_retries => {
                    attempt += 1;
                    fuchsia_async::Timer::new(retry_delay).await;
                }
                Err(err) => return Err(err),
            }
        };

        if is_final {
            offset = total_size;
        } else {
            let max_expected = offset + bytes_read as u64;
            let committed = next_offset.unwrap_or(max_expected);
            if committed <= offset || committed > max_expected {
                bail!(
                    "GCS resumable upload reported invalid next offset {committed} after uploading {offset}..{max_expected}"
                );
            }
            offset = committed;
        }
    }

    Ok(())
}

/// Authenticated HTTP transport and JSON serialization layer.
#[derive(Debug)]
struct HttpClient {
    access_token: String,
    https_client: HttpsClient,
}

impl HttpClient {
    fn new(access_token: String) -> Self {
        Self { access_token, https_client: new_https_client() }
    }

    async fn send_request(
        &self,
        method: Method,
        url: Url,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        allowed_statuses: &[StatusCode],
    ) -> Result<(StatusCode, hyper::HeaderMap, hyper::body::Bytes)> {
        let mut builder = Request::builder().method(&method).uri(url.as_str());

        if !self.access_token.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", self.access_token));
        }

        for &(k, v) in headers {
            builder = builder.header(k, v);
        }

        let req = builder.body(Body::from(body)).context("Failed to build HTTP request")?;
        let res = self.https_client.request(req).await.context("HTTP request failed")?;
        let status = res.status();
        let res_headers = res.headers().clone();

        let collected = res.into_body().collect().await.context("Failed to read response body")?;
        let bytes = collected.to_bytes();

        if !status.is_success() && !allowed_statuses.contains(&status) {
            let error_text = String::from_utf8_lossy(&bytes);
            bail!("API request to {} failed with status {}: {}", url, status, error_text);
        }

        Ok((status, res_headers, bytes))
    }

    async fn send_raw(
        &self,
        method: Method,
        url: Url,
        body: Option<(&str, Vec<u8>)>,
        allowed_statuses: &[StatusCode],
    ) -> Result<hyper::body::Bytes> {
        let (headers, body_bytes) = match body {
            Some((content_type, bytes)) => {
                let len_str = bytes.len().to_string();
                let (_, _, res_bytes) = self
                    .send_request(
                        method,
                        url,
                        &[("Content-Type", content_type), ("Content-Length", &len_str)],
                        bytes,
                        allowed_statuses,
                    )
                    .await?;
                return Ok(res_bytes);
            }
            None => {
                let headers: &[(&str, &str)] = if method == Method::POST {
                    &[("Content-Type", "application/json"), ("Content-Length", "0")]
                } else {
                    &[]
                };
                (headers, Vec::new())
            }
        };

        let (_, _, res_bytes) =
            self.send_request(method, url, headers, body_bytes, allowed_statuses).await?;
        Ok(res_bytes)
    }

    async fn start_resumable_upload(
        &self,
        url: Url,
        content_type: &str,
        total_size: u64,
    ) -> Result<Url> {
        let total_size_str = total_size.to_string();
        let headers = [
            ("Content-Type", "application/json; charset=UTF-8"),
            ("Content-Length", "0"),
            ("X-Upload-Content-Type", content_type),
            ("X-Upload-Content-Length", &total_size_str),
        ];
        let (_, res_headers, _) =
            self.send_request(Method::POST, url, &headers, Vec::new(), &[]).await?;
        let location = res_headers
            .get(hyper::header::LOCATION)
            .ok_or_else(|| {
                anyhow::anyhow!("GCS resumable upload response missing Location header")
            })?
            .to_str()
            .context("GCS Location header is not valid UTF-8")?;
        Url::parse(location).context("Failed to parse GCS resumable upload session URL")
    }

    async fn put_upload_chunk(
        &self,
        session_url: Url,
        content_type: &str,
        content_range: &str,
        chunk: Vec<u8>,
        is_final: bool,
    ) -> Result<Option<u64>> {
        let len_str = chunk.len().to_string();
        let headers = [
            ("Content-Type", content_type),
            ("Content-Length", &len_str),
            ("Content-Range", content_range),
        ];
        let allowed_statuses: &[StatusCode] =
            if is_final { &[] } else { &[StatusCode::PERMANENT_REDIRECT] };
        let (status, res_headers, _) =
            self.send_request(Method::PUT, session_url, &headers, chunk, allowed_statuses).await?;

        if status == StatusCode::PERMANENT_REDIRECT {
            let next_offset = res_headers
                .get(hyper::header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_range_header);
            return Ok(next_offset);
        }

        Ok(None)
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<Vec<u8>>,
    ) -> Result<T> {
        let payload = body.map(|b| ("application/json", b));
        let bytes = self.send_raw(method, url, payload, &[]).await?;
        serde_json::from_slice(&bytes).context("Failed to parse JSON response")
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        self.send_json(Method::GET, url, None).await
    }

    async fn post_json<B: Serialize, T: DeserializeOwned>(&self, url: Url, body: &B) -> Result<T> {
        let bytes = serde_json::to_vec(body)?;
        self.send_json(Method::POST, url, Some(bytes)).await
    }

    async fn post_empty_json<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        self.send_json(Method::POST, url, None).await
    }

    async fn delete_json<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        self.send_json(Method::DELETE, url, None).await
    }

    async fn post_raw(
        &self,
        url: Url,
        content_type: &str,
        body: Vec<u8>,
        allowed_statuses: &[StatusCode],
    ) -> Result<()> {
        self.send_raw(Method::POST, url, Some((content_type, body)), allowed_statuses).await?;
        Ok(())
    }

    async fn delete_raw(&self, url: Url, allowed_statuses: &[StatusCode]) -> Result<()> {
        self.send_raw(Method::DELETE, url, None, allowed_statuses).await?;
        Ok(())
    }
}

/// Stateless endpoint URL construction for GCE and GCS REST APIs.
mod endpoints {
    use super::{Context, Result, Url};

    const COMPUTE_BASE: &str = "https://compute.googleapis.com/compute/v1";
    const STORAGE_BASE: &str = "https://storage.googleapis.com/storage/v1";
    const STORAGE_UPLOAD_BASE: &str = "https://storage.googleapis.com/upload/storage/v1";

    fn build_url(base: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<Url> {
        let mut url = Url::parse(base).context("Invalid base URL")?;
        url.path_segments_mut().map_err(|_| anyhow::anyhow!("Invalid base URL"))?.extend(segments);
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query {
                pairs.append_pair(k, v);
            }
        }
        Ok(url)
    }

    pub fn images(project: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "global", "images"], &[])
    }

    pub fn image(project: &str, image_name: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "global", "images", image_name], &[])
    }

    pub fn instances(project: &str, zone: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "zones", zone, "instances"], &[])
    }

    pub fn list_instances(project: &str, zone: &str) -> Result<Url> {
        instances(project, zone)
    }

    pub fn instance(project: &str, zone: &str, instance_name: &str) -> Result<Url> {
        build_url(
            COMPUTE_BASE,
            &["projects", project, "zones", zone, "instances", instance_name],
            &[],
        )
    }

    pub fn start_instance(project: &str, zone: &str, instance_name: &str) -> Result<Url> {
        build_url(
            COMPUTE_BASE,
            &["projects", project, "zones", zone, "instances", instance_name, "start"],
            &[],
        )
    }

    pub fn stop_instance(project: &str, zone: &str, instance_name: &str) -> Result<Url> {
        build_url(
            COMPUTE_BASE,
            &["projects", project, "zones", zone, "instances", instance_name, "stop"],
            &[],
        )
    }

    pub fn serial_port_output(
        project: &str,
        zone: &str,
        instance_name: &str,
        port: u32,
        start: Option<i64>,
    ) -> Result<Url> {
        let port_str = port.to_string();
        let start_str = start.map(|s| s.to_string());
        let mut query = vec![("port", port_str.as_str())];
        if let Some(ref s) = start_str {
            query.push(("start", s.as_str()));
        }
        build_url(
            COMPUTE_BASE,
            &["projects", project, "zones", zone, "instances", instance_name, "serialPort"],
            &query,
        )
    }

    pub fn global_operation(project: &str, op_name: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "global", "operations", op_name], &[])
    }

    pub fn zone_operation(project: &str, zone: &str, op_name: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "zones", zone, "operations", op_name], &[])
    }

    pub fn firewalls(project: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "global", "firewalls"], &[])
    }

    pub fn firewall_rule(project: &str, rule_name: &str) -> Result<Url> {
        build_url(COMPUTE_BASE, &["projects", project, "global", "firewalls", rule_name], &[])
    }

    pub fn create_bucket(project: &str) -> Result<Url> {
        build_url(STORAGE_BASE, &["b"], &[("project", project)])
    }

    pub fn upload_gcs_object(bucket: &str, object_name: &str) -> Result<Url> {
        build_url(
            STORAGE_UPLOAD_BASE,
            &["b", bucket, "o"],
            &[("uploadType", "resumable"), ("name", object_name)],
        )
    }

    pub fn delete_gcs_object(bucket: &str, object_name: &str) -> Result<Url> {
        build_url(STORAGE_BASE, &["b", bucket, "o", object_name], &[])
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, endpoints, parse_range_header, upload_reader_resumable};
    use std::cell::RefCell;
    use std::io::Cursor;
    use std::rc::Rc;

    #[fuchsia::test]
    fn test_image_url_construction() {
        let url = endpoints::image("test-p", "test-img").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/global/images/test-img"
        );
    }

    #[fuchsia::test]
    fn test_images_url_construction() {
        let url = endpoints::images("test-p").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/global/images"
        );
    }

    #[fuchsia::test]
    fn test_list_instances_url_construction() {
        let url = endpoints::list_instances("test-p", "test-z").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances"
        );
    }

    #[fuchsia::test]
    fn test_get_instance_url_construction() {
        let url = endpoints::instance("test-p", "test-z", "test-inst").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances/test-inst"
        );
    }

    #[fuchsia::test]
    fn test_get_serial_port_output_url_construction() {
        let url =
            endpoints::serial_port_output("test-p", "test-z", "test-inst", 1, Some(100)).unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances/test-inst/serialPort?port=1&start=100"
        );

        let url_no_start =
            endpoints::serial_port_output("test-p", "test-z", "test-inst", 1, None).unwrap();
        assert_eq!(
            url_no_start.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances/test-inst/serialPort?port=1"
        );
    }

    #[fuchsia::test]
    fn test_start_instance_url_construction() {
        let url = endpoints::start_instance("test-p", "test-z", "test-inst").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances/test-inst/start"
        );
    }

    #[fuchsia::test]
    fn test_stop_instance_url_construction() {
        let url = endpoints::stop_instance("test-p", "test-z", "test-inst").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances/test-inst/stop"
        );
    }

    #[fuchsia::test]
    fn test_delete_instance_url_construction() {
        let url = endpoints::instance("test-p", "test-z", "test-inst").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/instances/test-inst"
        );
    }

    #[fuchsia::test]
    fn test_global_operation_url_construction() {
        let url = endpoints::global_operation("test-p", "operation-123").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/global/operations/operation-123"
        );
    }

    #[fuchsia::test]
    fn test_zone_operation_url_construction() {
        let url = endpoints::zone_operation("test-p", "test-z", "operation-123").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/zones/test-z/operations/operation-123"
        );
    }

    #[fuchsia::test]
    fn test_firewall_rule_url_construction() {
        let url = endpoints::firewall_rule("test-p", "test-firewall").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/global/firewalls/test-firewall"
        );
    }

    #[fuchsia::test]
    fn test_firewalls_url_construction() {
        let url = endpoints::firewalls("test-p").unwrap();
        assert_eq!(
            url.as_str(),
            "https://compute.googleapis.com/compute/v1/projects/test-p/global/firewalls"
        );
    }

    #[fuchsia::test]
    fn test_create_bucket_url_construction() {
        let url = endpoints::create_bucket("my-project").unwrap();
        assert_eq!(url.as_str(), "https://storage.googleapis.com/storage/v1/b?project=my-project");
    }

    #[fuchsia::test]
    fn test_upload_gcs_url_construction() {
        let url = endpoints::upload_gcs_object("my-bucket", "folder/item.tar.gz").unwrap();
        assert_eq!(
            url.as_str(),
            "https://storage.googleapis.com/upload/storage/v1/b/my-bucket/o?uploadType=resumable&name=folder%2Fitem.tar.gz"
        );
    }

    #[fuchsia::test]
    fn test_delete_gcs_url_construction() {
        let url = endpoints::delete_gcs_object("my-bucket", "folder/item.tar.gz").unwrap();
        assert_eq!(
            url.as_str(),
            "https://storage.googleapis.com/storage/v1/b/my-bucket/o/folder%2Fitem.tar.gz"
        );
    }

    #[fuchsia::test]
    fn test_parse_range_header() {
        assert_eq!(parse_range_header("bytes=0-262143"), Some(262144));
        assert_eq!(parse_range_header(" bytes=0-0 "), Some(1));
        assert_eq!(parse_range_header("invalid"), None);
        assert_eq!(parse_range_header("bytes=100"), None);
    }

    #[fuchsia::test]
    async fn test_upload_reader_resumable_multi_chunk() {
        let data: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        let total_size = data.len() as u64;
        let mut cursor = Cursor::new(data.clone());

        let recorded_ranges = Rc::new(RefCell::new(Vec::new()));
        let assembled = Rc::new(RefCell::new(Vec::new()));

        let ranges_clone = recorded_ranges.clone();
        let assembled_clone = assembled.clone();

        upload_reader_resumable(
            &mut cursor,
            total_size,
            256,
            3,
            Duration::from_millis(1),
            move |range, chunk, is_final| {
                ranges_clone.borrow_mut().push((range, chunk.len(), is_final));
                assembled_clone.borrow_mut().extend_from_slice(&chunk);
                async move { Ok(None) }
            },
        )
        .await
        .expect("upload should succeed");

        let ranges = recorded_ranges.borrow();
        assert_eq!(
            *ranges,
            vec![
                ("bytes 0-255/600".to_string(), 256, false),
                ("bytes 256-511/600".to_string(), 256, false),
                ("bytes 512-599/600".to_string(), 88, true),
            ]
        );
        assert_eq!(*assembled.borrow(), data);
    }

    #[fuchsia::test]
    async fn test_upload_reader_resumable_partial_range_and_retry() {
        let data: Vec<u8> = (0..100).collect();
        let total_size = data.len() as u64;
        let mut cursor = Cursor::new(data.clone());

        let calls = Rc::new(RefCell::new(Vec::new()));
        let calls_clone = calls.clone();

        upload_reader_resumable(
            &mut cursor,
            total_size,
            50,
            3,
            Duration::from_millis(1),
            move |range, chunk, is_final| {
                let call_idx = {
                    let mut c = calls_clone.borrow_mut();
                    c.push((range, chunk, is_final));
                    c.len()
                };
                async move {
                    match call_idx {
                        // First chunk (0..50): server commits only 0..29 (next offset = 30)
                        1 => Ok(Some(30)),
                        // Second chunk (30..80): simulate transient network failure
                        2 => Err(anyhow::anyhow!("transient network failure")),
                        // Retry of second chunk (30..80): succeeds completely
                        3 => Ok(Some(80)),
                        // Final chunk (80..100): succeeds
                        4 => Ok(None),
                        _ => unreachable!(),
                    }
                }
            },
        )
        .await
        .expect("upload with partial range and retry should succeed");

        let recorded = calls.borrow();
        assert_eq!(recorded.len(), 4);
        assert_eq!(recorded[0].0, "bytes 0-49/100");
        assert_eq!(recorded[1].0, "bytes 30-79/100");
        assert_eq!(recorded[2].0, "bytes 30-79/100");
        assert_eq!(recorded[3].0, "bytes 80-99/100");
        assert!(recorded[3].2);
    }

    #[fuchsia::test]
    async fn test_upload_reader_resumable_empty_file() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let recorded_clone = recorded.clone();

        upload_reader_resumable(
            &mut cursor,
            0,
            256,
            1,
            Duration::from_millis(1),
            move |range, chunk, is_final| {
                recorded_clone.borrow_mut().push((range, chunk.len(), is_final));
                async move { Ok(None) }
            },
        )
        .await
        .expect("empty upload should succeed");

        assert_eq!(*recorded.borrow(), vec![("bytes */0".to_string(), 0, true)]);
    }

    #[fuchsia::test]
    fn test_ssh_firewall_rule_name() {
        assert_eq!(super::ssh_firewall_rule_name("default"), "allow-ssh-ingress-default");
        assert_eq!(
            super::ssh_firewall_rule_name("global/networks/default"),
            "allow-ssh-ingress-default"
        );
        assert_eq!(
            super::ssh_firewall_rule_name("global/networks/custom-net/"),
            "allow-ssh-ingress-custom-net"
        );
        assert_eq!(super::ssh_firewall_rule_name(""), "allow-ssh-ingress-default");
        assert_eq!(super::ssh_firewall_rule_name("///"), "allow-ssh-ingress-default");
    }
}
