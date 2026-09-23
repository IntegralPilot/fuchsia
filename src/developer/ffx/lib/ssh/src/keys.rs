// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use base64::display::Base64Display;
use base64::prelude::{BASE64_STANDARD, Engine as _};
use ffx_config::EnvironmentContext;
use ffx_config::api::ConfigError;
use ffx_config::keys::{AUTHORIZED_KEYS_HTTP_PORT_QUERY, SSH_PRIVATE_KEY, SSH_PUB_KEY};
use fho::FfxContext;
use fuchsia_async::Task;
use http_body_util::{BodyExt, Empty};
use hyper::body::Buf;
use hyper_util::rt::TokioIo;
use ring::rand::{self, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::iter::Iterator;
use std::net::TcpStream;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::{env, fmt, str};
use tokio::net::TcpStream as AsyncTcpStream;

fn auth_keys_filter_map(source: SshKeySource, line: &str) -> Option<SshKey> {
    // Note: only ed25519 should be extracted. If this changes, just append to here.
    const SSH_KEY_TYPES: [&'static str; 1] = ["ssh-ed25519"];
    let not_comments = !line.is_empty() && !line.starts_with('#');
    let parts: Vec<&str> = line.split_whitespace().collect();
    let sufficient_parts = parts.len() >= 2;
    if not_comments && sufficient_parts {
        let key_type_idx = parts.iter().position(|p| SSH_KEY_TYPES.iter().any(|t| t == p))?;
        let key_type = parts.get(key_type_idx)?.to_string();
        // There should always be a key after this idx.
        let key = BASE64_STANDARD.decode(parts.get(key_type_idx + 1)?).ok()?;
        let comment = if parts.len() > key_type_idx + 1 {
            Some(parts[(key_type_idx + 2)..].join(" "))
        } else {
            None
        };
        let mut sources = HashSet::new();
        sources.insert(source);
        Some(SshKey { key_type, key, comment, sources })
    } else {
        None
    }
}

async fn query_device_authorized_keys(
    mut addr: std::net::SocketAddr,
    port: u16,
) -> fho::Result<HashSet<SshKey>> {
    let context_string = "While querying device for SSH authorized_keys,";
    addr.set_port(port);
    let stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).with_user_message(|| {
            format!("{context_string} we were unable to connect a TCP stream to {addr:?}")
        })?;
    stream.set_nonblocking(true).bug()?;
    let stream = AsyncTcpStream::from_std(stream).bug()?;
    let (mut request_sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| {
            fho::user_error!(
                "{context_string} failed to initiate http handshake with device at {addr}: {e:?}"
            )
        })?;
    let _conn_task = Task::local(connection);
    let response = request_sender
        .send_request(
            hyper::Request::builder()
                .header("Host", format!("{addr}"))
                .method("GET")
                .body(Empty::<hyper::body::Bytes>::new())
                .bug()?,
        )
        .await
        .map_err(|e| {
            fho::user_error!("{context_string} failed to send HTTP request to {addr}: {e:?}")
        })?;
    let status = response.status();
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| fho::user_error!("failed to read http body: {e:?}"))?
        .to_bytes();
    if status != hyper::StatusCode::OK {
        let body = String::from_utf8_lossy(&body_bytes);
        fho::return_user_error!(
            "{context_string} received an HTTP error from {addr}: {status}. Body: \"{body}\""
        );
    }
    let body_str = String::from_utf8_lossy(body_bytes.chunk());
    if body_str.is_empty() {
        fho::return_user_error!(
            "Received empty authorized_keys file from {addr}. You may need to reflash this device."
        );
    }
    Ok(body_str
        .lines()
        .filter_map(|line| auth_keys_filter_map(SshKeySource::Device, line))
        .collect())
}

/// The successful result of [find_matching_ssh_keys]. Contains information about the errors
/// encountered attempting to read the ssh keys, a guaranteed non-empty list of keys, and the
/// directories searched when looking for ssh keys.
pub struct MatchingKeysInfo {
    pub keys: HashSet<SshKey>,
    pub dirs_searched: Vec<PathBuf>,
    pub io_errors: IoErrors,
}

/// Attempts to download ssh keys from the remote address via HTTP. Forces usage of port 9797.
///
/// This will return a list of SSH keys (if `Ok(_)` it is guaranteed to be non-empty).
/// If no keys are found that match the authorized_keys on the fuchsia device, an error will be
/// returned detailing the directories inspected, and what public keys were read.
// This doesn't use the target info holder to prevent a circular dependency. Furthermore we're
// trying to avoid using the FIDL structures too much in internal code.
pub async fn find_matching_ssh_keys(
    ctx: &EnvironmentContext,
    addr: std::net::SocketAddr,
) -> fho::Result<MatchingKeysInfo> {
    let http_port: u16 = ctx
        .get(AUTHORIZED_KEYS_HTTP_PORT_QUERY)
        .user_message("Unable to load authorized_keys port from config")?;
    let local_ssh_dirs = local_ssh_key_dirs(ctx)?;
    let (local_keys, io_errors) = get_ssh_public_keys(&local_ssh_dirs)?;
    Ok(MatchingKeysInfo {
        keys: find_matching_ssh_keys_impl(local_ssh_dirs.clone(), local_keys, addr, http_port)
            .await?,
        dirs_searched: local_ssh_dirs,
        io_errors,
    })
}

// Helper method to make testing easier.
async fn find_matching_ssh_keys_impl(
    local_ssh_dirs: Vec<PathBuf>,
    mut local_keys: HashSet<SshKey>,
    addr: std::net::SocketAddr,
    port: u16,
) -> fho::Result<HashSet<SshKey>> {
    let device_authorized_keys = query_device_authorized_keys(addr, port).await?;
    let mut found_keys = HashSet::new();
    for device_key in device_authorized_keys.iter() {
        // We're using `take` here because `local_keys` contains the file sources, which
        // is for guiding the user (to show that there are multiple sources from whence the
        // ssh keys came).
        if let Some(k) = local_keys.take(&device_key) {
            found_keys.insert(k);
        }
    }

    // The majority of the logic here is just making something displayable for the user to read in
    // the full error formatting.
    if found_keys.is_empty() {
        let device_keys = device_authorized_keys
            .iter()
            .map(|key| format!("{key}"))
            .collect::<Vec<_>>()
            .join("\n-- ");
        let ssh_agent_info = ssh_agent_keys_message(&local_keys);
        let non_agent_keys_msg = local_non_agent_keys_message(&local_ssh_dirs, &local_keys);
        fho::return_user_error!(
            "None of the following device SSH public keys matched any local ssh keys:\n-- {}\n\n{}\n\n{}

You may need to reflash the device or reconfigure your SSH agent. Please consult
https://fuchsia.dev/fuchsia-src/development/tools/ffx/workflows/create-ssh-keys-for-devices
for more details",
            device_keys,
            ssh_agent_info,
            non_agent_keys_msg,
        );
    }
    Ok(found_keys)
}

// Turns the searched directories into an info message for when we didn't find any ssh public keys
// that matched locally.
fn local_non_agent_keys_message(
    searched_dirs: &Vec<PathBuf>,
    local_keys: &HashSet<SshKey>,
) -> String {
    let non_agent_keys = local_keys
        .iter()
        .filter_map(|k| {
            if k.sources.is_empty() {
                None
            } else {
                let mut k = k.clone();
                let dirs = std::mem::replace(&mut k.sources, Default::default());
                Some(format!("{k}\n\t-- Found in {dirs:?}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n-- ");

    if non_agent_keys.is_empty() {
        format!(
            "No local ssh keys were found. We looked in the following locations:\n-- {}",
            searched_dirs
                .iter()
                .map(|d| format!("{}", d.display()))
                .collect::<Vec<_>>()
                .join("\n-- ")
        )
    } else {
        format!(
            "When searching local directories, we found the following public keys:\n-- {}",
            non_agent_keys
        )
    }
}

// Turns the ssh agent keys into an info message showing all the ones found (or if none were found)
// in the event that we didn't find any matching ssh public keys.
fn ssh_agent_keys_message(local_keys: &HashSet<SshKey>) -> String {
    let ssh_agent_keys = local_keys
        .iter()
        .filter_map(|k| {
            if k.sources.iter().any(|source| matches!(source, SshKeySource::SshAgent)) {
                Some(format!("{k}"))
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n-- ");
    if ssh_agent_keys.is_empty() {
        format!("No public keys were found from the ssh-agent")
    } else {
        format!("When querying the ssh-agent we found the following keys:\n-- {ssh_agent_keys}")
    }
}

fn fuchsia_ssh_key_dir() -> Option<PathBuf> {
    Some(PathBuf::from(env::var("FUCHSIA_DIR").ok()?).join(".ssh"))
}

fn local_ssh_key_dirs(ctx: &EnvironmentContext) -> fho::Result<Vec<PathBuf>> {
    let mut dirs = HashSet::from([
        // Regular SSH directory on *nix systems.
        PathBuf::from(env::var("HOME").user_message("Could not find home directory")?).join(".ssh"),
    ]);
    if let Some(fuchsia_dir) = fuchsia_ssh_key_dir() {
        dirs.insert(fuchsia_dir);
    }
    let mut configured_dirs = ctx
        .get::<Vec<PathBuf>, _>(SSH_PUB_KEY)
        .user_message("Could not load ssh.pub file from config")?;
    // For certain setups no public key will be set, only the private key location.
    configured_dirs.extend(
        ctx.get::<Vec<PathBuf>, _>(SSH_PRIVATE_KEY)
            .user_message("Could not load ssh.priv file from config")?,
    );
    // Look for the directory for each entry, not the file.
    for d in configured_dirs.iter_mut() {
        d.pop();
    }
    dirs.extend(configured_dirs);
    Ok(dirs.into_iter().collect())
}

fn find_keys_in_dirs(
    dirs: &Vec<PathBuf>,
    file_filter: impl Fn(&PathBuf) -> bool + Copy,
    file_parser: impl Fn(String, &PathBuf) -> Vec<SshKey>,
) -> (Vec<SshKey>, IoErrors) {
    let mut keys = Vec::new();
    let mut io_errors = IoErrors::new();
    for dir in dirs {
        let read_dir = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                io_errors.push(dir.clone(), e);
                continue;
            }
        };
        for pub_key_file in read_dir
            .into_iter()
            .filter_map(|entry_res| entry_res.ok())
            .map(|e| e.path())
            .filter(file_filter)
        {
            let content = match fs::read_to_string(&pub_key_file) {
                Ok(c) => c,
                Err(e) => {
                    io_errors.push(pub_key_file, e);
                    continue;
                }
            };
            keys.extend((file_parser)(content, &pub_key_file));
        }
    }
    (keys, io_errors)
}

fn find_ssh_keys_in_dirs(dirs: &Vec<PathBuf>) -> (SshKeySet, IoErrors) {
    let mut keys = SshKeySet::new();
    // Find all public keys.
    let (public_keys, errs1) = find_keys_in_dirs(
        dirs,
        |path| {
            path.is_file()
                && path.extension().and_then(|s| s.to_str()).map_or(false, |ext| ext == "pub")
        },
        |content, source_file| {
            content
                .lines()
                .filter_map(|line| {
                    auth_keys_filter_map(SshKeySource::File(source_file.clone()), line)
                })
                .collect()
        },
    );
    // Find all private keys.
    let (private_keys, errs2) = find_keys_in_dirs(
        dirs,
        |path| path.is_file() && path.extension().is_none(),
        |content, source_file| match read_public_key_from_private_string(content) {
            Ok((key_type, headless_key)) => {
                // This header is padding zeroes, then 0xb and "ssh-ed25519". The above function
                // returns the public key with no header included.
                let mut key = vec![
                    0x00, 0x00, 0x00, 0x0b, 0x73, 0x73, 0x68, 0x2d, 0x65, 0x64, 0x32, 0x35, 0x35,
                    0x31, 0x39, 0x00, 0x00, 0x00, 0x20,
                ];
                key.extend(headless_key);
                vec![SshKey {
                    key_type,
                    key,
                    comment: None,
                    sources: [SshKeySource::File(source_file.clone())].into_iter().collect(),
                }]
            }
            Err(e) => {
                log::debug!("Unable to read SSH file {}: {e}", source_file.display());
                Vec::new()
            }
        },
    );
    public_keys.into_iter().chain(private_keys.into_iter()).for_each(|k| keys.insert(k));
    (keys, errs1.into_iter().chain(errs2.into_iter()).collect::<Vec<_>>().into())
}

fn get_ssh_agent_identities() -> SshKeySet {
    let mut keys = SshKeySet::new();
    match Command::new("ssh-add").arg("-L").output() {
        Ok(res) => {
            if res.status.success() {
                let stdout_str = String::from_utf8_lossy(&res.stdout);
                stdout_str
                    .lines()
                    .filter_map(|line| auth_keys_filter_map(SshKeySource::SshAgent, line))
                    .for_each(|k| {
                        keys.insert(k);
                    });
            }
        }
        Err(_e) => {}
    }
    keys
}

/// Tries to find all public keys from both the ssh-agent and from local directories.
/// an `Ok(_)` result is guaranteed to be non-empty. Not finding any SSH keys will result in
/// returning an error.
fn get_ssh_public_keys(dirs: &Vec<PathBuf>) -> fho::Result<(HashSet<SshKey>, IoErrors)> {
    let (local_keys, io_errors) = find_ssh_keys_in_dirs(dirs);
    let agent_keys = get_ssh_agent_identities();
    let local_keys = local_keys.union(agent_keys);
    let local_keys = local_keys.into_hashset();
    if local_keys.is_empty() {
        fho::return_user_error!(
            "Unable to locate local SSH keys from either the ssh agent or any of the following directories:\n-- {}",
            dirs.iter().map(|d| format!("{}", d.display())).collect::<Vec<_>>().join("\n-- ")
        );
    }
    Ok((local_keys, io_errors))
}

/// A structure for tracking SSH keys. This makes sure that as keys are added, their overall paths
/// are tracked. After inserting all keys, this can be converted into a HashSet where each key will
/// contain all the directories in which the key can be found.
struct SshKeySet {
    inner: HashMap<SshKey, HashSet<SshKeySource>>,
}

impl SshKeySet {
    fn new() -> Self {
        Self { inner: Default::default() }
    }

    fn insert(&mut self, mut key: SshKey) {
        let sources = std::mem::replace(&mut key.sources, Default::default());
        self.inner
            .entry(key)
            .and_modify(|d| {
                d.extend(sources.clone());
            })
            .or_insert(sources);
    }

    fn into_hashset(self) -> HashSet<SshKey> {
        let res = self
            .inner
            .into_iter()
            .map(|(mut k, v)| {
                k.sources.extend(v);
                k
            })
            .collect();
        res
    }

    fn union(mut self, other: Self) -> Self {
        for (mut key, dirs) in other.inner.into_iter() {
            key.sources.extend(dirs);
            self.insert(key)
        }
        self
    }
}

#[derive(Debug)]
pub struct IoErrors {
    inner: Vec<(PathBuf, std::io::Error)>,
}

impl IoErrors {
    fn new() -> Self {
        Self { inner: Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn push(&mut self, p: PathBuf, e: std::io::Error) {
        self.inner.push((p, e))
    }

    pub fn into_iter(self) -> std::vec::IntoIter<(PathBuf, std::io::Error)> {
        self.inner.into_iter()
    }
}

impl From<Vec<(PathBuf, std::io::Error)>> for IoErrors {
    fn from(other: Vec<(PathBuf, std::io::Error)>) -> Self {
        Self { inner: other }
    }
}

#[derive(serde::Serialize, Eq, PartialEq, Clone, Hash)]
pub enum SshKeySource {
    File(PathBuf),
    SshAgent,
    Device,
}

impl Display for SshKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let display = match self {
            Self::File(pb) => format!("{pb:?}"),
            Self::SshAgent => "agent".to_owned(),
            Self::Device => "device".to_owned(),
        };
        write!(f, "{display}")
    }
}

impl Debug for SshKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        // Just use Display impl.
        write!(f, "{self}")
    }
}

#[derive(serde::Serialize, Eq, Clone, Debug)]
pub struct SshKey {
    pub key_type: String,
    pub key: Vec<u8>,
    pub comment: Option<String>,
    pub sources: HashSet<SshKeySource>,
}

impl Hash for SshKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Comment/source is not relevant for hashing.
        self.key_type.hash(state);
        self.key.hash(state);
    }
}

impl PartialEq for SshKey {
    fn eq(&self, other: &Self) -> bool {
        self.key_type == other.key_type && self.key == other.key
    }
}

impl fmt::Display for SshKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "key type: {}, key: {}{}",
            self.key_type,
            BASE64_STANDARD.encode(self.key.as_slice()),
            match self.sources.len() {
                0_usize => format!(""),
                1_usize => {
                    format!(", found in: {:?}", self.sources.iter().next().unwrap())
                }
                2_usize.. => {
                    format!(", found in: {:?}", self.sources)
                }
            }
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub enum SshKeyErrorKind {
    BadKeyType,
    BadBase64Encoding,
    BadConfiguration,
    BadFilePermission,
    BadKeyFormat,
    BadUTFEncoding,
    GenerationError,
    KeyAlreadyExists,
    IOError,
    KeyMismatch,
    FileNotFound,
}

impl fmt::Display for SshKeyErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone)]
struct SshKeyInternalError {
    pub kind: SshKeyErrorKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct SshKeyError {
    pub kind: SshKeyErrorKind,
    pub message: String,
}
impl fmt::Display for SshKeyInternalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.message)
    }
}
impl fmt::Display for SshKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.message)
    }
}

impl Error for SshKeyError {}

impl From<std::io::Error> for SshKeyInternalError {
    fn from(value: std::io::Error) -> Self {
        match value.kind() {
            std::io::ErrorKind::AlreadyExists => SshKeyInternalError {
                kind: SshKeyErrorKind::KeyAlreadyExists,
                message: format!("{value:?}"),
            },
            _ => SshKeyInternalError {
                kind: SshKeyErrorKind::IOError,
                message: format!("{value:?}"),
            },
        }
    }
}

impl From<str::Utf8Error> for SshKeyInternalError {
    fn from(value: str::Utf8Error) -> Self {
        SshKeyInternalError { kind: SshKeyErrorKind::BadUTFEncoding, message: format!("{value:?}") }
    }
}

impl From<base64::DecodeError> for SshKeyInternalError {
    fn from(value: base64::DecodeError) -> Self {
        SshKeyInternalError {
            kind: SshKeyErrorKind::BadBase64Encoding,
            message: format!("{value:?}"),
        }
    }
}

impl From<ConfigError> for SshKeyError {
    fn from(value: ConfigError) -> Self {
        SshKeyError { kind: SshKeyErrorKind::BadConfiguration, message: format!("{value:?}") }
    }
}

impl From<SshKeyInternalError> for SshKeyError {
    fn from(value: SshKeyInternalError) -> Self {
        let kind = match value.kind {
            SshKeyErrorKind::BadBase64Encoding | SshKeyErrorKind::BadUTFEncoding => {
                SshKeyErrorKind::BadKeyFormat
            }
            _ => value.kind,
        };
        SshKeyError { kind: kind, message: value.message }
    }
}

/// Paths to the private and public SSH keys that are used by ffx.
/// These can be loaded from the configuration keys `ssh.pub`
/// and `ssh.priv` by using SshKeyFiles::load().
/// Typical usage is to load from the configuration and then create the keys
/// if they are missing when initializing a device via flashing/paving
///  or starting an emulator instance.
///
/// let ssh_keys = SshKeyFiles::load().await?;
/// ssh_keys.create_keys_if_needed()?;
///
/// This is preferred since generating the private key when attempting to access
/// a device already initialized is pointless.
///
///
#[derive(Debug, Default)]
pub struct SshKeyFiles {
    pub authorized_keys: PathBuf,
    pub private_key: PathBuf,
}

const KEYTYPE_STR: &str = "ssh-ed25519";
const KEYTYPE: &[u8] = b"ssh-ed25519";
const COMMENT: &str = "Generated by ffx for Fuchsia";
const AUTH_MAGIC: &[u8] = b"openssh-key-v1\0";

impl SshKeyFiles {
    /// loads the file paths from the config properties `ssh.pub` and `ssh.priv`.
    /// If none of the paths configured are to files that exist, the paths will
    ///  be set to the default sources, which is the first element in the config settings.
    pub fn load(ctx: &EnvironmentContext) -> Result<Self, SshKeyError> {
        // initialize to the first path in the list, then iterate through the list to select
        // the first file that exists.
        let authorized_keys_files: Vec<PathBuf> = ctx.get(SSH_PUB_KEY)?;
        if authorized_keys_files.is_empty() {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::BadConfiguration,
                message: "No paths configured for `ssh.pub`.".into(),
            });
        }
        let mut authorized_keys = authorized_keys_files[0].to_path_buf();
        for path in &authorized_keys_files {
            if path.exists() {
                authorized_keys = path.to_path_buf();
                break;
            }
        }

        let key_files: Vec<PathBuf> = ctx.get(SSH_PRIVATE_KEY)?;
        if key_files.is_empty() {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::BadConfiguration,
                message: format!("No paths configured for `ssh.priv`."),
            });
        }
        let mut private_key = key_files[0].to_path_buf();
        for path in &key_files {
            if path.exists() {
                private_key = path.to_path_buf();
                break;
            }
        }
        Ok(SshKeyFiles { authorized_keys, private_key })
    }

    /// Generates the ed25519 key pair and saves openssh authorized_keys and private key file,
    /// if the paths point to files that do not exist.
    /// if append is true, the public key matching self.private key is appended to
    /// the self.authorized_keys file. There is no check for duplication, so
    /// append should only be true if check_keys returns KeyMismatch.
    pub fn create_keys_if_needed(&self, append: bool) -> Result<(), SshKeyError> {
        let mut public_key: Vec<u8> = vec![];
        let mut do_write_public_key = append;

        // Validate the paths are non-empty
        if self.private_key.display().to_string().is_empty() {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::BadConfiguration,
                message: "private key path cannot be empty".into(),
            });
        }
        if self.authorized_keys.display().to_string().is_empty() {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::BadConfiguration,
                message: "authorized keys path cannot be empty".into(),
            });
        }

        if !self.private_key.exists() {
            // There is no private key, so generate a new key pair
            // resulting in a byte array .der encoded.
            log::info!("Creating SSH key pair: {}", self.private_key.display());
            let rng = SystemRandom::new();
            let bytes = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|m| SshKeyError {
                kind: SshKeyErrorKind::GenerationError,
                message: format!("could not generate pkcs8 document: {:?}", m),
            })?;
            let key_pair = Ed25519KeyPair::from_pkcs8(bytes.as_ref()).map_err(|m| SshKeyError {
                kind: SshKeyErrorKind::GenerationError,
                message: format!("could not get keypair from pkcs8 document: {:?}", m),
            })?;

            // If somehow between the check and now the key is created, return early
            match write_private_key(&self.private_key, &key_pair, bytes.as_ref(), &rng) {
                Ok(_) => {}
                Err(e) if e.kind == SshKeyErrorKind::KeyAlreadyExists => {
                    log::debug!("Key already exists in create_keys_if_needed");
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            };
            public_key = key_pair.public_key().as_ref().to_vec();
            do_write_public_key = true;
        } else if do_write_public_key || !self.authorized_keys.exists() {
            // If we get here we need to get the public key from the private key.
            // If reading fails, the file is corrupted or it is not the format expected.
            // The easiest corrective action would be for the user to delete the file or generate authorized keys using ssh-keygen.
            // Since there is no way to know if the key is being used, an error is returned since   there is no safe way to continue
            // and make sure the keys are valid.
            let key_type: String;
            (key_type, public_key) = read_public_key_from_private(&self.private_key)?;

            //write out the public key, only if the key type is OK
            // ffx by default uses ed25519 keys. If the private key is something else (for example rsa), print
            // bail giving instructions on how to generate the public key.
            if key_type != KEYTYPE_STR {
                return Err(SshKeyError {
                    kind: SshKeyErrorKind::BadKeyType,
                    message: format!("The private key in {priv} is type {key_type}. This program can only verify ed25529 keys.\
            \n To re-add the public key to {auth}, run\
            \n ssh-keygen -y -f {priv} >> {auth}", priv=self.private_key.to_string_lossy(), auth=self.authorized_keys.to_string_lossy()),
                });
            }
            do_write_public_key = true;
        }

        if do_write_public_key {
            // check to write the authorized keys file for when the private existed, and when it was generated.
            write_public_key(&self.authorized_keys, &public_key).map_err(|e| SshKeyError {
                kind: SshKeyErrorKind::IOError,
                message: format!("{e}"),
            })?;
        }

        Ok(())
    }

    /// Checks the validity of the public and private key files.
    /// |repair_if_needed| is true, then any problems with the keys
    /// are corrected if possible.
    /// If it is not possible, an error is returned, and the recommended
    /// course of action is to delete the keys and try again.
    pub fn check_keys(&self, repair_if_needed: bool) -> Result<String, SshKeyError> {
        if !repair_if_needed {
            self.analyze_check_keys()?;
            log::info!("SSH Public/Private keys match");
            return Ok("SSH Public/Private keys match".into());
        }

        let mut messages: Vec<String> = Vec::new();
        let mut push_message = |msg: String| {
            if !messages.contains(&msg) {
                messages.push(msg);
            }
        };

        // Allow multiple repair steps because fixing one issue can uncover the next (e.g.,
        // repairing file permissions -> detecting invalid key format and regenerating private key ->
        // appending matching public key to authorized_keys). A maximum of 3 distinct repair steps
        // can be chained; 5 attempts provide a small safeguard against infinite loops.
        const MAX_REPAIR_ATTEMPTS: usize = 5;
        for _ in 0..MAX_REPAIR_ATTEMPTS {
            match self.analyze_check_keys() {
                Ok(_) => {
                    log::info!("SSH Public/Private keys match");
                    if messages.is_empty() {
                        return Ok("SSH Public/Private keys match".into());
                    } else {
                        return Ok(format!("Keys repaired: {}.", messages.join(", ")));
                    }
                }
                Err(e) => match e.kind {
                    // Non-repairable errors: return immediately without modifying files.
                    SshKeyErrorKind::BadKeyType | SshKeyErrorKind::BadConfiguration => {
                        return Err(e);
                    }

                    // Bad Format. The private key file is corrupted or unreadable.
                    // Delete it and regenerate fresh keys.
                    SshKeyErrorKind::BadKeyFormat => {
                        if self.private_key.exists() {
                            fs::remove_file(&self.private_key).map_err(|io_err| SshKeyError {
                                kind: SshKeyErrorKind::IOError,
                                message: format!("Cannot delete {:?}: {io_err}", self.private_key),
                            })?;
                        }
                        self.create_keys_if_needed(false)?;
                        push_message(format!("{}. Regenerating a private key", e.message));
                    }

                    // Bad File permission. This means the private key file permission is
                    // wrong. Fix it.
                    SshKeyErrorKind::BadFilePermission => {
                        let meta = self.private_key.metadata().map_err(|io_err| SshKeyError {
                            kind: SshKeyErrorKind::IOError,
                            message: format!("{io_err}"),
                        })?;
                        let mut permissions = meta.permissions();
                        permissions.set_mode(0o600);
                        fs::set_permissions(&self.private_key, permissions).map_err(|io_err| {
                            SshKeyError {
                                kind: SshKeyErrorKind::IOError,
                                message: format!("{io_err}"),
                            }
                        })?;
                        push_message(format!(
                            "Repaired file permissions for {}",
                            self.private_key.display()
                        ));
                    }

                    // Key Mismatch. The private key is valid, but matching public key is not in authorized_keys.
                    SshKeyErrorKind::KeyMismatch => {
                        self.create_keys_if_needed(true)?;
                        push_message(e.message);
                    }

                    // File not found or any other error: create missing keys.
                    _ => {
                        self.create_keys_if_needed(false)?;
                        push_message(e.message);
                    }
                },
            }
        }

        // If after multiple repair steps it still fails, run analyze_check_keys one last time
        // to return the remaining error.
        self.analyze_check_keys()?;
        Ok(format!("Keys repaired: {}.", messages.join(", ")))
    }

    /// Checks that the corresponding public key from the private key file
    /// is listed in the authorized keys file.
    fn analyze_check_keys(&self) -> Result<(), SshKeyError> {
        if !self.private_key.exists() {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::FileNotFound,
                message: format!(
                    "Private key {} does not exist",
                    self.private_key.to_string_lossy()
                ),
            });
        } else {
            let meta = self.private_key.metadata().map_err(|e| SshKeyError {
                kind: SshKeyErrorKind::IOError,
                message: format!("{e}"),
            })?;
            let mode = meta.permissions().mode();
            if mode != 0o100600 {
                return Err(SshKeyError {
                    kind: SshKeyErrorKind::BadFilePermission,
                    message: format!(
                        "Private key {} has the wrong file permissions. SSH requires 0o600, found {mode:#o}",
                        self.private_key.display()
                    ),
                });
            }
        }
        let (key_type, public_key) = match read_public_key_from_private(&self.private_key) {
            Ok((key_type, public_key)) => (key_type, public_key),
            Err(e) => {
                log::debug!("Internal error for read_public_key_from_private: {e:?}");
                return Err(SshKeyError {
                    kind: SshKeyErrorKind::BadKeyFormat,
                    message: format!(
                        "Could not read OpenSSH Ed25519 private key from {}: {}. Only unencrypted OpenSSH Ed25519 keys are supported; check `ssh.priv` configuration or replace the key file",
                        self.private_key.display(),
                        e.message
                    ),
                });
            }
        };
        let entry = build_public_key_entry(&key_type, &public_key).map_err(|e| SshKeyError {
            kind: SshKeyErrorKind::BadKeyFormat,
            message: format!(
                "Failed to format public key for private key {}: {e}",
                self.private_key.display()
            ),
        })?;

        // ffx by default uses ed25519 keys. If the private key is something else (for example rsa), print
        // bail giving instructions on how to generate the public key.
        if key_type != KEYTYPE_STR {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::BadKeyType,
                message: format!("The private key in {priv} is type {key_type}. This program can only verify ed25529 keys.\
            \n To re-add the public key to {auth}, run\
            \n ssh-keygen -y -f {priv} >> {auth}", priv=self.private_key.to_string_lossy(), auth=self.authorized_keys.to_string_lossy()),
            });
        }

        if !self.authorized_keys.exists() {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::FileNotFound,
                message: format!(
                    "Authorized key file {} does not exist",
                    self.authorized_keys.to_string_lossy()
                ),
            });
        }

        let file = File::open(&self.authorized_keys)
            .map_err(|e| SshKeyError { kind: SshKeyErrorKind::IOError, message: format!("{e}") })?;
        // Read the file line by line, and return an iterator of the lines of the file.
        // Note: This check is only for the keys that could be generated by ad-hoc or by ffx directly and used
        // on the target Fuchsia device to secure the ssh connection to the device.
        // Specifically, we're loooking for a ssh-ed25519 key type that is not part of a key ring. Optional
        // fields before the key type are not expected.
        if !BufReader::new(file)
            .lines()
            .into_iter()
            .map(|l| l.unwrap())
            .any(|l| l.starts_with(&entry))
        {
            return Err(SshKeyError {
                kind: SshKeyErrorKind::KeyMismatch,
                message: format!(
                    "Could not find matching public key for the private key {}",
                    self.private_key.to_string_lossy()
                ),
            });
        }
        Ok(())
    }
}

/// Formats the key data for the authorized_keys file.
fn get_public_key_data(pubkey: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut out_bytes: Vec<u8> = vec![];

    // public key is 2 "cstrings", which are strings with no null terminator preceded by the
    // length.
    write_cstring(&mut out_bytes, KEYTYPE)?;
    write_cstring(&mut out_bytes, pubkey)?;

    Ok(out_bytes)
}

/// Builds the authorized_keys entry for the given public key.
fn build_public_key_entry(key_type: &str, public_key: &[u8]) -> Result<String, std::io::Error> {
    let public_key_data = get_public_key_data(public_key)?;
    let pubkey_b64 =
        Base64Display::new(&public_key_data, &base64::engine::general_purpose::STANDARD);
    Ok(format!("{} {}", key_type, pubkey_b64))
}

/// Appends the public key information to the authorized_keys file.
fn write_public_key(path: &PathBuf, public_key: &[u8]) -> Result<(), SshKeyInternalError> {
    log::info!("Writing authorized_keys file: {}", path.display());

    let mut w = if !path.exists() {
        if let Some(parent) = path.parent() {
            DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        };
        OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(&path)?
    } else {
        // append to the file.
        OpenOptions::new().write(true).append(true).open(&path)?
    };
    writeln!(
        &mut w,
        "{} {}",
        build_public_key_entry(str::from_utf8(KEYTYPE)?, public_key)?,
        COMMENT
    )?;
    // File is closed when it goes out of scope.
    Ok(())
}

/// Writes the private key file.
fn write_private_key(
    path: &PathBuf,
    key_pair: &Ed25519KeyPair,
    document: &[u8],
    rng: &SystemRandom,
) -> Result<(), SshKeyInternalError> {
    // private key file
    let none = b"none";

    // magic pattern to identify this data, null terminated.
    let mut priv_out_bytes: Vec<u8> = vec![];
    priv_out_bytes.write_all(AUTH_MAGIC)?;

    // ciphername
    write_cstring(&mut priv_out_bytes, none)?;

    // kdfname (none), and length 0
    write_cstring(&mut priv_out_bytes, none)?;
    priv_out_bytes.write_all(&[0, 0, 0, 0])?;

    // number of keys, always 1.
    priv_out_bytes.write_all(&[0, 0, 0, 1])?;

    // public key - this is the same contents as appears in the authorized_keys file.
    let public_key_data = get_public_key_data(key_pair.public_key().as_ref())?;
    write_cstring(&mut priv_out_bytes, &public_key_data)?;

    // private key.
    let mut key_bytes: Vec<u8> = vec![];

    // random u32 checkbytes, write it 2 times.
    let rand_bytes: [u8; 4] = rand::generate(rng).unwrap().expose();
    key_bytes.write_all(&rand_bytes)?;
    key_bytes.write_all(&rand_bytes)?;

    // The type of key.
    write_cstring(&mut key_bytes, KEYTYPE)?;

    // Extract the secret part of the key from the pkcs8 document.
    // the first 16 bytes are the version and algorithm oid. The private
    // key data starts at 16, and is 32 bytes
    // secret key, should be 32 bytes.
    let secret = &document[16..48];

    // pub key 32 bytes.
    write_cstring(&mut key_bytes, key_pair.public_key().as_ref())?;

    // the private key is the secret with the public appended for a
    // total of 64 bytes.
    let mut private_key_data: Vec<u8> = Vec::from(secret);
    private_key_data.extend_from_slice(key_pair.public_key().as_ref());
    write_cstring(&mut key_bytes, &private_key_data)?;

    // add the comment.
    write_cstring(&mut key_bytes, COMMENT.as_bytes())?;

    // padding
    let mut i: u8 = 0;
    while key_bytes.len() % 8 != 0 {
        i += 1;
        key_bytes.write_all(&[i])?;
    }

    write_cstring(&mut priv_out_bytes, &key_bytes)?;

    let begin = "-----BEGIN OPENSSH PRIVATE KEY-----\n";
    let end = "-----END OPENSSH PRIVATE KEY-----\n";

    if let Some(parent) = path.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    };
    let mut w =
        OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(&path)?;
    writeln!(&mut w, "{}", begin)?;
    writeln!(
        &mut w,
        "{}",
        Base64Display::new(&priv_out_bytes, &base64::engine::general_purpose::STANDARD)
    )?;
    writeln!(&mut w, "{}", end)?;
    // File is closed when it goes out of scope.
    Ok(())
}

fn read_public_key_from_private_string(
    content: String,
) -> Result<(String, Vec<u8>), SshKeyInternalError> {
    let mut started = false;
    let mut encoded: String = String::from("");
    for line in content.lines() {
        if line.starts_with("----") && line.contains("BEGIN OPENSSH PRIVATE KEY") {
            started = true;
            continue;
        }
        if line.starts_with("----") && line.contains("END OPENSSH PRIVATE KEY") {
            // done
            break;
        }
        if started {
            // append all lines be between begin and end, trimming whitespace.
            encoded.push_str(line.trim());
        }
    }
    // decode the base64 string into bytes.
    let data = BASE64_STANDARD.decode(&encoded)?;
    let mut buf = Cursor::new(data);

    let mut element: Vec<u8> = vec![];

    // read the magic, it is null terminated.
    buf.read_until(0, &mut element)?;
    if element != AUTH_MAGIC {
        return Err(SshKeyInternalError {
            kind: SshKeyErrorKind::BadKeyFormat,
            message: format!("Invalid private key header {:?}", element),
        });
    }

    // read cipher and kdf settings, both none.
    element = read_cstring(&mut buf)?;
    if "none" != str::from_utf8(&element)? {
        return Err(SshKeyInternalError {
            kind: SshKeyErrorKind::BadKeyFormat,
            message: format!("Invalid private key header, expected 'none' {:?}", element),
        });
    }
    element = read_cstring(&mut buf)?;
    if "none" != str::from_utf8(&element)? {
        return Err(SshKeyInternalError {
            kind: SshKeyErrorKind::BadKeyFormat,
            message: format!("Invalid private key header, expected 'none' {:?}", element),
        });
    }
    let mut u32_bytes = [0u8; 4];
    buf.read_exact(&mut u32_bytes)?;
    if u32::from_be_bytes(u32_bytes) != 0 {
        return Err(SshKeyInternalError {
            kind: SshKeyErrorKind::BadKeyFormat,
            message: format!("Invalid private key header, expected 0, got {:?}", u32_bytes),
        });
    }

    // read number of keys, should only be 1.
    buf.read_exact(&mut u32_bytes)?;
    if u32::from_be_bytes(u32_bytes) != 1 {
        return Err(SshKeyInternalError {
            kind: SshKeyErrorKind::BadKeyFormat,
            message: format!("Invalid private key count, expected 1, got {:?}", u32_bytes),
        });
    }

    // read the public key data
    element = read_cstring(&mut buf)?;

    // this is keytype|key. Read the type, then return the key
    let mut keydata = Cursor::new(&element);
    let key_type = read_cstring(&mut keydata)?;
    let pubkey = read_cstring(&mut keydata)?;

    Ok((str::from_utf8(&key_type)?.to_string(), pubkey))
}

/// Reads the public key from the private key file.
fn read_public_key_from_private(path: &PathBuf) -> Result<(String, Vec<u8>), SshKeyInternalError> {
    let priv_key_file = File::open(path)?;
    let mut contents = String::new();
    let _ = BufReader::new(priv_key_file).read_to_string(&mut contents)?;
    read_public_key_from_private_string(contents)
}

fn write_cstring(buf: &mut dyn Write, bytes: &[u8]) -> Result<(), std::io::Error> {
    let len: u32 = bytes.len().try_into().expect("usize cast to u32");
    buf.write_all(&len.to_be_bytes())?;
    buf.write_all(bytes)?;
    Ok(())
}

fn read_cstring(buf: &mut dyn Read) -> Result<Vec<u8>, std::io::Error> {
    let mut size = [0u8; 4];
    buf.read_exact(&mut size)?;
    let len = u32::from_be_bytes(size);
    if len > 0 {
        let sz: usize = len.try_into().unwrap();
        let mut ret: Vec<u8> = vec![0; sz];
        buf.read_exact(&mut ret)?;
        return Ok(ret);
    }
    Ok(vec![])
}

#[cfg(test)]
mod test {
    use super::*;

    use std::io::{Read, Write};
    use tempfile::TempDir;

    #[fuchsia::test]
    fn test_load() {
        let env = ffx_config::test_env()
            .user_config(
                SSH_PUB_KEY,
                vec!["$ENV_PATH_THAT_IS_NOT_SET", "/expected/default", "someother"],
            )
            .user_config(
                SSH_PRIVATE_KEY,
                vec!["$ENV_PATH_THAT_IS_NOT_SET_2", "/expected/default/private", "someother/place"],
            )
            .build()
            .expect("test env init");

        let ssh_files = match SshKeyFiles::load(&env.context) {
            Ok(ssh) => ssh,
            Err(e) => panic!("load failed: {e:?}"),
        };
        assert!(&ssh_files.authorized_keys.display().to_string() == "/expected/default");
        assert!(&ssh_files.private_key.display().to_string() == "/expected/default/private");
    }

    #[test]
    fn test_enum_display() {
        let v = SshKeyErrorKind::BadKeyFormat;
        assert_eq!(format!("{v}"), "BadKeyFormat")
    }
    #[test]
    fn test_create_with_existing() {
        let tmp_dir = TempDir::new().expect("create temp dir");

        let auth_key_path = tmp_dir.path().join("authorized_keys");
        let private_path = tmp_dir.path().join("privatekey");

        // scope to force the file to close.
        {
            let mut tmp_file = File::create(&auth_key_path).expect("create authorized ");
            let test_private_key = include_str!("../testdata/test1_ed25519");
            tmp_file.write_all(b"unchanged\n").expect("write authorized keys bytes");
            let mut priv_file = File::create(&private_path).expect("create private key path");
            priv_file.write_all(test_private_key.as_bytes()).expect("write private key bytes");
        }

        let ssh_files = SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path };
        if let Err(e) = ssh_files.create_keys_if_needed(false) {
            panic!("create_keys_if_needed failed: {e:?}");
        }

        let contents = fs::read_to_string(ssh_files.authorized_keys).expect("read authorized keys");
        let lines: Vec<&str> = contents.lines().collect();

        // existing keys should not be modified by create_keys_if_needed.
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "unchanged");
    }

    #[test]
    fn test_create_with_missing_auth_keys() {
        let tmp_dir = TempDir::new().expect("create temp dir");

        let auth_key_path = tmp_dir.path().join("authorized_keys");
        let private_path = tmp_dir.path().join("privatekey");

        // scope to force the file to close.
        {
            let test_private_key = include_str!("../testdata/test1_ed25519");
            let mut priv_file = File::create(&private_path).expect("Create priv key path");
            priv_file.write_all(test_private_key.as_bytes()).expect("write contents of priv key");
        }

        let ssh_files = SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path };
        if let Err(e) = ssh_files.create_keys_if_needed(false) {
            panic!("create_keys_if_needed failed: {e:?}");
        }

        let contents =
            fs::read_to_string(ssh_files.authorized_keys).expect("read authorized_keys contents");
        let expected_contents = include_str!("../testdata/test1_authorized_keys");

        assert!(contents == expected_contents);
    }

    #[test]
    fn test_create_with_missing_keys() {
        let tmp_dir = TempDir::new().expect("create temp dir");

        let auth_key_path = tmp_dir.path().join("authorized_keys");
        let private_path = tmp_dir.path().join("privatekey");

        assert!(!&auth_key_path.exists());
        assert!(!&private_path.exists());

        let ssh_files = SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path };
        if let Err(e) = ssh_files.create_keys_if_needed(false) {
            panic!("create_keys_if_needed failed: {e:?}");
        }

        assert!(&ssh_files.authorized_keys.exists());
        assert!(&ssh_files.private_key.exists());
    }

    #[test]
    fn test_write_cstring() {
        let mut data = vec![];
        let mut expected_data: Vec<u8> = vec![0, 0, 0, 5];
        expected_data.extend_from_slice("hello".as_bytes());
        if let Err(e) = write_cstring(&mut data, "hello".as_bytes()) {
            panic!("write_cstring failed: {e:?}");
        }

        assert!(data == expected_data);

        let mut input = Cursor::new(data);
        let read_data = match read_cstring(&mut input) {
            Ok(d) => d,
            Err(e) => panic!("read_cstring failed: {e:?}"),
        };
        assert!(read_data == "hello".as_bytes());
    }

    #[test]
    fn test_create_with_missing_directory_for_keys() {
        let tmp_dir = TempDir::new().expect("creating temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");

        assert!(!&auth_key_path.exists());
        assert!(!&private_path.exists());

        let ssh_files = SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path };
        match ssh_files.create_keys_if_needed(false) {
            Ok(_) => (),
            Err(e) => panic!("create keys if needed error: {e:?}"),
        };

        assert!(&ssh_files.authorized_keys.exists());
        assert!(&ssh_files.private_key.exists());
    }

    #[test]
    fn test_check_keys() {
        let tmp_dir = TempDir::new().expect("creating temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");

        assert!(!&auth_key_path.exists());
        assert!(!&private_path.exists());

        let ssh_files = SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path };
        ssh_files.create_keys_if_needed(false).expect("creating test keys");

        match ssh_files.check_keys(false) {
            Ok(_) => (),
            Err(e) => panic!("create keys if needed error: {e:?}"),
        };
    }

    #[test]
    fn test_check_keys_missing() {
        let tmp_dir = TempDir::new().expect("creating temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");

        assert!(!&auth_key_path.exists());
        assert!(!&private_path.exists());

        let ssh_files = SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path };

        match ssh_files.check_keys(false) {
            Ok(_) => panic!("missing keys should fail"),
            Err(e) => assert_eq!(e.kind, SshKeyErrorKind::FileNotFound, "{e:?}"),
        }
    }

    #[test]
    fn test_check_keys_mismatch() {
        let tmp_dir = TempDir::new().expect("create temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");
        let other_auth_key_path = new_dir_path.join("other_authorized_keys");
        let other_private_path = new_dir_path.join("other_privatekey");

        assert!(!&auth_key_path.exists());
        assert!(!&private_path.exists());

        let ssh_files =
            SshKeyFiles { authorized_keys: auth_key_path.clone(), private_key: private_path };
        match ssh_files.create_keys_if_needed(false) {
            Ok(_) => (),
            Err(e) => panic!("create keys if needed error: {e:?}"),
        };

        let other_ssh_files = SshKeyFiles {
            authorized_keys: other_auth_key_path,
            private_key: other_private_path.clone(),
        };
        match other_ssh_files.create_keys_if_needed(false) {
            Ok(_) => (),
            Err(e) => panic!("create keys if needed error: {e:?}"),
        };

        let mismatched =
            SshKeyFiles { authorized_keys: auth_key_path, private_key: other_private_path };

        match mismatched.check_keys(false) {
            Ok(_) => panic!("mismatched keys should fail"),
            Err(e) => assert_eq!(e.kind, SshKeyErrorKind::KeyMismatch, "{e:?}"),
        };
    }

    #[test]
    fn test_check_keys_mismatch_repaired() {
        let tmp_dir = TempDir::new().expect("create temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");
        let other_auth_key_path = new_dir_path.join("other_authorized_keys");
        let other_private_path = new_dir_path.join("other_privatekey");

        assert!(!&auth_key_path.exists());
        assert!(!&private_path.exists());

        let ssh_files =
            SshKeyFiles { authorized_keys: auth_key_path.clone(), private_key: private_path };
        match ssh_files.create_keys_if_needed(false) {
            Ok(_) => (),
            Err(e) => panic!("create keys if needed error: {e:?}"),
        };

        let other_ssh_files = SshKeyFiles {
            authorized_keys: other_auth_key_path,
            private_key: other_private_path.clone(),
        };
        match other_ssh_files.create_keys_if_needed(false) {
            Ok(_) => (),
            Err(e) => panic!("create keys if needed error: {e:?}"),
        };

        let mismatched =
            SshKeyFiles { authorized_keys: auth_key_path, private_key: other_private_path.clone() };

        match mismatched.check_keys(true) {
            Ok(message) => assert_eq!(
                message,
                format!(
                    "Keys repaired: Could not find matching public key for the private key {}.",
                    other_private_path.to_string_lossy()
                )
            ),
            Err(e) => assert_eq!(e.kind, SshKeyErrorKind::KeyMismatch, "{e:?}"),
        };

        //check the mismatched keys are OK.
        match mismatched.check_keys(false) {
            Ok(_) => (),
            Err(e) => panic!("unexpected error {e} for mismatched keys"),
        };

        // other should be ok since nothing should have changed.
        match other_ssh_files.check_keys(false) {
            Ok(_) => (),
            Err(e) => panic!("unexpected error {e} for other keys"),
        }

        // ssh_keys should shill be OK too.
        match ssh_files.check_keys(false) {
            Ok(_) => (),
            Err(e) => panic!("unexpected error {e} ssh keys"),
        }
    }

    #[test]
    fn test_check_keys_bad_format() {
        let tmp_dir = TempDir::new().expect("creating temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");

        fs::create_dir_all(&new_dir_path).expect("create dir");
        fs::write(&auth_key_path, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG... comment\n")
            .expect("write auth key");
        fs::write(&private_path, "not an openssh private key").expect("write priv key");

        let mut perms = fs::metadata(&private_path).expect("metadata").permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&private_path, perms).expect("set perms");

        let ssh_files =
            SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path.clone() };

        // check_keys without repair should fail with BadKeyFormat
        match ssh_files.check_keys(false) {
            Ok(_) => panic!("bad key format should fail"),
            Err(e) => {
                assert_eq!(e.kind, SshKeyErrorKind::BadKeyFormat, "{e:?}");
                assert!(e.message.contains(&*private_path.to_string_lossy()));
            }
        }

        // check_keys with repair_if_needed = true should repair by regenerating a fresh key
        match ssh_files.check_keys(true) {
            Ok(message) => {
                assert!(message.contains("Keys repaired:"));
                assert!(message.contains("Regenerating a private key."));
            }
            Err(e) => panic!("repairing bad key format failed: {e:?}"),
        }

        // After repair, check_keys should pass
        match ssh_files.check_keys(false) {
            Ok(_) => (),
            Err(e) => panic!("check_keys after repair failed: {e:?}"),
        }

        // Verify the private key is now a valid key file, not the old invalid content
        assert!(private_path.exists());
        let contents = fs::read_to_string(&private_path).expect("read priv key");
        assert_ne!(contents, "not an openssh private key");
        assert!(contents.contains("BEGIN OPENSSH PRIVATE KEY"));
    }

    #[test]
    fn test_check_keys_bad_permission_repair() {
        let tmp_dir = TempDir::new().expect("creating temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");

        let ssh_files =
            SshKeyFiles { authorized_keys: auth_key_path, private_key: private_path.clone() };
        ssh_files.create_keys_if_needed(false).expect("creating test keys");

        // Change permissions to 0644 (wrong permission)
        let mut perms = fs::metadata(&private_path).expect("metadata").permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&private_path, perms).expect("set perms");

        // check_keys without repair should fail with BadFilePermission
        match ssh_files.check_keys(false) {
            Ok(_) => panic!("bad permission should fail"),
            Err(e) => assert_eq!(e.kind, SshKeyErrorKind::BadFilePermission, "{e:?}"),
        }

        // check_keys with repair should fix the permissions
        ssh_files.check_keys(true).expect("repair permissions");

        let mode = fs::metadata(&private_path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // After repair, check_keys should pass
        ssh_files.check_keys(false).expect("check keys after permission repair");
    }

    #[test]
    fn test_check_keys_touched_file_multi_repair() {
        let tmp_dir = TempDir::new().expect("creating temp dir");

        let new_dir_path = tmp_dir.path().join("new-dir");
        let auth_key_path = new_dir_path.join("authorized_keys");
        let private_path = new_dir_path.join("privatekey");

        fs::create_dir_all(&new_dir_path).expect("create dir");
        // Simulate `touch privatekey`: empty file with 0644 permissions
        fs::write(&private_path, "").expect("touch private key");
        let mut perms = fs::metadata(&private_path).expect("metadata").permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&private_path, perms).expect("set perms");

        let ssh_files = SshKeyFiles {
            authorized_keys: auth_key_path.clone(),
            private_key: private_path.clone(),
        };

        // check_keys without repair fails with BadFilePermission
        match ssh_files.check_keys(false) {
            Ok(_) => panic!("touched file should fail"),
            Err(e) => assert_eq!(e.kind, SshKeyErrorKind::BadFilePermission, "{e:?}"),
        }

        // check_keys with repair should fix permissions, detect empty format, and regenerate valid keys in ONE pass
        let repair_msg = ssh_files.check_keys(true).expect("repair touched file in one pass");
        assert!(repair_msg.contains("Repaired file permissions"));
        assert!(repair_msg.contains("Regenerating a private key"));

        // After single repair invocation, check_keys without repair passes immediately
        ssh_files.check_keys(false).expect("check keys after repair");

        // Verify valid private key and authorized_keys on disk
        let mode = fs::metadata(&private_path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let contents = fs::read_to_string(&private_path).expect("read priv key");
        assert!(contents.contains("BEGIN OPENSSH PRIVATE KEY"));
        assert!(auth_key_path.exists());
    }

    #[test]
    fn test_ssh_key_set_insert() {
        let mut key_set = SshKeySet::new();
        let mut key1 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment1".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir1"))]),
        };

        key_set.insert(key1.clone());

        // The key should be in the inner map, and its sources should contain /dir1
        let mut expected_dirs = HashSet::new();
        expected_dirs.insert(SshKeySource::File(PathBuf::from("/dir1")));
        key1.sources.clear(); // insert clears sources
        assert_eq!(key_set.inner.get(&key1), Some(&expected_dirs));

        // Insert the same key but with a different parent directory
        let key2 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment2".to_string()), // comment is ignored for hashing
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir2"))]),
        };
        key_set.insert(key2.clone());

        // The sources for key1 should now contain both /dir1 and /dir2
        expected_dirs.insert(SshKeySource::File(PathBuf::from("/dir2")));
        assert_eq!(key_set.inner.get(&key1), Some(&expected_dirs));
    }

    #[test]
    fn test_ssh_key_set_into_hashset() {
        let mut key_set = SshKeySet::new();
        let key1 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment1".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir1"))]),
        };
        let key2 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment2".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir2"))]),
        };
        let key3 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key2data").unwrap(),
            comment: None,
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir3"))]),
        };

        key_set.insert(key1);
        key_set.insert(key2);
        key_set.insert(key3);

        let hash_set = key_set.into_hashset();

        assert_eq!(hash_set.len(), 2);

        let expected_key1 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment1".to_string()), // The comment of the first inserted key is kept.
            sources: HashSet::from([
                SshKeySource::File(PathBuf::from("/dir1")),
                SshKeySource::File(PathBuf::from("/dir2")),
            ]),
        };
        let expected_key2 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key2data").unwrap(),
            comment: None,
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir3"))]),
        };

        assert!(hash_set.contains(&expected_key1));
        assert!(hash_set.contains(&expected_key2));

        // Check sources of the found key
        let found_key1 = hash_set.get(&expected_key1).unwrap();
        assert_eq!(found_key1.sources, expected_key1.sources);
    }

    #[test]
    fn test_ssh_key_set_union() {
        let mut key_set1 = SshKeySet::new();
        let key1 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment1".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir1"))]),
        };
        let key2 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key2data").unwrap(),
            comment: None,
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir2"))]),
        };
        key_set1.insert(key1);
        key_set1.insert(key2);

        let mut key_set2 = SshKeySet::new();
        let key3 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment3".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir3"))]),
        };
        let key4 = SshKey {
            key_type: "ssh-dss".to_string(),
            key: BASE64_STANDARD.decode("key3data").unwrap(),
            comment: None,
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir4"))]),
        };
        key_set2.insert(key3);
        key_set2.insert(key4);

        let union_set = key_set1.union(key_set2);
        let hash_set = union_set.into_hashset();

        assert_eq!(hash_set.len(), 3);

        let expected_key1 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key1data").unwrap(),
            comment: Some("comment1".to_string()),
            sources: HashSet::from([
                SshKeySource::File(PathBuf::from("/dir1")),
                SshKeySource::File(PathBuf::from("/dir3")),
            ]),
        };
        let expected_key2 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("key2data").unwrap(),
            comment: None,
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir2"))]),
        };
        let expected_key3 = SshKey {
            key_type: "ssh-dss".to_string(),
            key: BASE64_STANDARD.decode("key3data").unwrap(),
            comment: None,
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir4"))]),
        };

        assert!(hash_set.contains(&expected_key1));
        assert!(hash_set.contains(&expected_key2));
        assert!(hash_set.contains(&expected_key3));

        let found_key1 = hash_set.get(&expected_key1).unwrap();
        assert_eq!(found_key1.sources, expected_key1.sources);
    }

    const TEST_DATA_DIR_1: &str = "../../src/developer/ffx/lib/ssh/testdata/key_parsing";
    const TEST_DATA_DIR_2: &str = "../../src/developer/ffx/lib/ssh/testdata/key_parsing/other_dir";

    #[test]
    fn test_find_ssh_keys() {
        let paths = vec![PathBuf::from(TEST_DATA_DIR_1), PathBuf::from(TEST_DATA_DIR_2)];
        let (keys, _) = find_ssh_keys_in_dirs(&paths);
        // There should only be two keys despite the directories and clones of one of the keys.
        assert_eq!(keys.inner.len(), 2);
        let keyset = keys.into_hashset();
        let cloned_key = keyset.iter().filter(|k| k.sources.len() == 3).next().unwrap();
        assert!(cloned_key.sources.iter().any(|loc| *loc
            == SshKeySource::File(PathBuf::from(
                "../../src/developer/ffx/lib/ssh/testdata/key_parsing/other_dir/key_other_copy.pub"
            ))));
        let unique_key = keyset.iter().filter(|k| k.sources.len() == 1).next().unwrap();
        assert!(
            *unique_key.sources.iter().next().unwrap()
                == SshKeySource::File(PathBuf::from(
                    "../../src/developer/ffx/lib/ssh/testdata/key_parsing/other_key.pub"
                ))
        )
    }

    // Helper to create a simple HTTP server.
    fn start_test_server(status: hyper::StatusCode, body: &'static str) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.shutdown(std::net::Shutdown::Write).unwrap();
                let mut buf = [0; 128];
                // Read until EOF to make sure client has received everything.
                while stream.read(&mut buf).unwrap_or(0) > 0 {}
            }
        });
        addr
    }

    #[fuchsia::test]
    async fn test_query_device_authorized_keys_error_response() {
        const ERROR_BODY: &str = "Something went wrong";
        let server_addr = start_test_server(hyper::StatusCode::INTERNAL_SERVER_ERROR, ERROR_BODY);

        let result = query_device_authorized_keys(server_addr, server_addr.port()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let expected_err_msg = format!(
            "While querying device for SSH authorized_keys, received an HTTP error from {}: 500 Internal Server Error. Body: \"{}\"",
            server_addr, ERROR_BODY
        );
        assert_eq!(err.to_string(), expected_err_msg);
    }

    #[fuchsia::test]
    async fn test_query_device_authorized_keys_success() {
        const FAKE_KEYS: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID/PA/k3f5aSU/22c9LPn/4A3f/g/2Yj/2c/8A3/g/2Y test1@fuchsia\n\
                                 ssh-ed25519 AAAA1234 test2@fuchsia";
        let server_addr = start_test_server(hyper::StatusCode::OK, FAKE_KEYS);

        let keys = query_device_authorized_keys(server_addr, server_addr.port()).await.unwrap();

        assert_eq!(keys.len(), 2);
        let key1 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD
                .decode("AAAAC3NzaC1lZDI1NTE5AAAAID/PA/k3f5aSU/22c9LPn/4A3f/g/2Yj/2c/8A3/g/2Y")
                .unwrap(),
            comment: Some("test1@fuchsia".to_string()),
            sources: HashSet::new(),
        };
        let key2 = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("AAAA1234").unwrap(),
            comment: Some("test2@fuchsia".to_string()),
            sources: HashSet::new(),
        };
        assert!(keys.contains(&key1));
        assert!(keys.contains(&key2));
    }

    #[fuchsia::test]
    async fn test_query_device_authorized_keys_empty_response() {
        let server_addr = start_test_server(hyper::StatusCode::OK, "");

        let result = query_device_authorized_keys(server_addr, server_addr.port()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Received empty authorized_keys file"));
    }

    #[fuchsia::test]
    async fn test_find_matching_ssh_keys_impl_success() {
        const DEVICE_KEYS: &str = "ssh-ed25519 akeyondeviceandlocal comment1\n\
                                   ssh-ed25519 localonlykey comment2";
        let server_addr = start_test_server(hyper::StatusCode::OK, DEVICE_KEYS);

        let mut local_keys = HashSet::new();
        let matching_key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("akeyondeviceandlocal").unwrap(),
            comment: Some("comment1".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/fake/path"))]),
        };
        let non_matching_key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("localonlykey1234").unwrap(),
            comment: Some("comment3".to_string()),
            sources: HashSet::new(),
        };
        local_keys.insert(matching_key.clone());
        local_keys.insert(non_matching_key);

        let local_ssh_dirs = vec![PathBuf::from("/fake/path")];

        let found_keys = find_matching_ssh_keys_impl(
            local_ssh_dirs,
            local_keys,
            server_addr,
            server_addr.port(),
        )
        .await
        .unwrap();

        assert_eq!(found_keys.len(), 1);
        assert!(found_keys.contains(&matching_key));
        let found_key = found_keys.iter().next().unwrap();
        assert_eq!(found_key.sources, matching_key.sources);
    }

    #[fuchsia::test]
    async fn test_find_matching_ssh_keys_impl_no_match() {
        const DEVICE_KEYS: &str = "ssh-ed25519 AAAA1234 comment1";
        let server_addr = start_test_server(hyper::StatusCode::OK, DEVICE_KEYS);

        let mut local_keys = HashSet::new();
        let non_matching_key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("localonlykey").unwrap(),
            comment: Some("comment2".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/fake/path"))]),
        };
        local_keys.insert(non_matching_key);

        let local_ssh_dirs = vec![PathBuf::from("/fake/path")];

        let result = find_matching_ssh_keys_impl(
            local_ssh_dirs,
            local_keys,
            server_addr,
            server_addr.port(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains(
                "None of the following device SSH public keys matched any local ssh keys"
            )
        );
        assert!(err.to_string().contains("AAAA1234"));
        assert!(err.to_string().contains("localonlykey"));
    }

    #[test]
    fn test_local_non_agent_keys_message_no_keys() {
        let searched_dirs = vec![PathBuf::from("/dir1"), PathBuf::from("/dir2")];
        let local_keys = HashSet::new();
        let message = local_non_agent_keys_message(&searched_dirs, &local_keys);
        assert_eq!(
            message,
            "No local ssh keys were found. We looked in the following locations:\n-- /dir1\n-- /dir2"
        );
    }

    #[test]
    fn test_local_non_agent_keys_message_with_keys() {
        let searched_dirs = vec![PathBuf::from("/dir1")];
        let mut local_keys = HashSet::new();
        let key_loc = PathBuf::from("/dir1/key.pub");
        let key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("AAAA2345").unwrap(),
            comment: Some("comment".to_string()),
            sources: HashSet::from([SshKeySource::File(key_loc.clone())]),
        };
        local_keys.insert(key);
        let message = local_non_agent_keys_message(&searched_dirs, &local_keys);
        let expected_key_str = "key type: ssh-ed25519, key: AAAA2345";
        let expected_msg = format!(
            "When searching local directories, we found the following public keys:\n-- {}\n\t-- Found in {:?}",
            expected_key_str,
            HashSet::from([key_loc])
        );
        assert_eq!(message, expected_msg);
    }

    #[test]
    fn test_ssh_agent_keys_message_only_paths() {
        let mut local_keys = HashSet::new();
        let key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("AAAA2345").unwrap(),
            comment: Some("comment".to_string()),
            sources: HashSet::from([SshKeySource::File(PathBuf::from("/dir1/key.pub"))]),
        };
        local_keys.insert(key);
        let message = ssh_agent_keys_message(&local_keys);
        assert_eq!(message, "No public keys were found from the ssh-agent");
    }

    #[test]
    fn test_ssh_agent_keys_message_with_no_sources() {
        let mut local_keys = HashSet::new();
        let key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("AAAA1234").unwrap(),
            comment: Some("agent comment".to_string()),
            sources: HashSet::new(),
        };
        local_keys.insert(key);
        let message = ssh_agent_keys_message(&local_keys);
        assert_eq!(message, "No public keys were found from the ssh-agent");
    }

    #[test]
    fn test_ssh_agent_keys_message_with_keys() {
        let mut local_keys = HashSet::new();
        let key = SshKey {
            key_type: "ssh-ed25519".to_string(),
            key: BASE64_STANDARD.decode("AAAA1234").unwrap(),
            comment: Some("agent comment".to_string()),
            sources: HashSet::from([SshKeySource::SshAgent]),
        };
        local_keys.insert(key);
        let message = ssh_agent_keys_message(&local_keys);
        let expected_key_str = "key type: ssh-ed25519, key: AAAA1234, found in: agent";
        let expected_msg = format!(
            "When querying the ssh-agent we found the following keys:\n-- {}",
            expected_key_str
        );
        assert_eq!(message, expected_msg);
    }

    const TEST_DATA_DIR_3: &str =
        "../../src/developer/ffx/lib/ssh/testdata/key_parsing/only_private_key";

    #[test]
    fn test_ssh_agent_keys_success_with_only_private_keys() {
        let paths = vec![PathBuf::from(TEST_DATA_DIR_3)];
        let (keys, _) = find_ssh_keys_in_dirs(&paths);
        // There should only be one key.
        assert_eq!(keys.inner.len(), 1);
        let keyset = keys.into_hashset();
        let private_key = keyset.iter().next().unwrap();
        assert!(private_key.sources.iter().any(|loc| *loc
            == SshKeySource::File(PathBuf::from(
                "../../src/developer/ffx/lib/ssh/testdata/key_parsing/only_private_key/only_private"
            ))));
        assert_eq!(
            private_key.key,
            BASE64_STANDARD
                .decode("AAAAC3NzaC1lZDI1NTE5AAAAIH5Tm3xmV3LGi0pNDiovSVpsoLEZiNSFMJN2wxnj8DnN")
                .unwrap()
        );
    }

    #[fuchsia::test]
    fn test_find_key_dirs_pub_and_private() {
        let env = ffx_config::test_env()
            .user_config(
                SSH_PUB_KEY,
                vec!["$ENV_PATH_THAT_IS_NOT_SET", "/expected/default", "/someother/thing"],
            )
            .user_config(
                SSH_PRIVATE_KEY,
                vec!["$ENV_PATH_THAT_IS_NOT_SET_2", "/expected/default/private", "someother/place"],
            )
            .build()
            .expect("test env init");

        let dirs = local_ssh_key_dirs(&env.context).unwrap();
        assert!(dirs.iter().any(|d| d.display().to_string() == "/expected/default"));
        assert!(dirs.iter().any(|d| d.display().to_string() == "/expected"));
        assert!(dirs.iter().any(|d| d.display().to_string() == "/someother"));
        assert!(dirs.iter().any(|d| d.display().to_string() == "someother"));
    }
}
