// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::LevelFilter;
use logging::{FfxLog, FfxLogSink, Filter, FormatOpts, LogSinkTrait, TargetsFilter, TestWriter};
use std::fs::{File, OpenOptions, create_dir_all, remove_file, rename};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, RwLock};

use crate::EnvironmentContext;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {0}")]
    Config(#[from] crate::ConfigError),

    #[error("opening log file: {0}")]
    OpenLog(#[source] std::io::Error),

    #[error("checking log file size: {0}")]
    CheckSize(#[source] std::io::Error),

    #[error("deleting stale log: {0}")]
    DeleteLog(#[source] std::io::Error),

    #[error("rotating log files: {0}")]
    RotateLog(#[source] std::io::Error),

    #[error("seeking through old log file: {0}")]
    SeekLog(#[source] std::io::Error),

    #[error("writing log truncation notice: {0}")]
    TruncateNotice(#[source] std::io::Error),

    #[error("reading old log file: {0}")]
    ReadOldLog(#[source] std::io::Error),

    #[error("writing truncated log file: {0}")]
    WriteTruncatedLog(#[source] std::io::Error),
}

/// The valid destinations for a log file
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogDestination {
    Stdout,
    Stderr,
    TestWriter,
    File(PathBuf),
    // "Global" means the global variable log file, which has special
    // semantics: it can change due to rotation, it can be swapped in
    // and out via change_log_file().
    Global,
}

impl std::str::FromStr for LogDestination {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "-" | "stdout" => Ok(LogDestination::Stdout),
            "stderr" => Ok(LogDestination::Stderr),
            _ => Ok(LogDestination::File(PathBuf::from(s))),
        }
    }
}

impl std::fmt::Display for LogDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Stdout => "stdout".to_string(),
            Self::Stderr => "stderr".to_string(),
            Self::TestWriter => "TestWriter".to_string(),
            Self::File(path) => {
                format!("File: {}", path.display())
            }
            Self::Global => "Global".to_string(),
        };
        write!(f, "{}", value)
    }
}

pub enum LogDirHandling {
    WithDirWithoutRotate,
    WithDirWithRotate,
}

pub const LOG_DIR: &str = "log.dir";
const LOG_ROTATIONS: &str = "log.rotations";
const LOG_ROTATE_SIZE: &str = "log.rotate_size";
const LOG_ENABLED: &str = "log.enabled";
const LOG_TARGET_LEVELS: &str = "log.target_levels";
const LOG_LEVEL: &str = "log.level";

// The log filename and basename should always be in-sync.
pub const LOG_FILENAME: &str = "ffx.log";
pub const LOG_BASENAME: &str = "ffx";

static LOG_ENABLED_FLAG: AtomicBool = AtomicBool::new(true);

pub static LOGGING_ID: LazyLock<u64> = LazyLock::new(generate_id);

pub fn disable_stdio_logging() {
    LOG_ENABLED_FLAG.store(false, Ordering::Relaxed);
}

fn generate_id() -> u64 {
    rand::random::<u64>()
}

// There are times when we want to change the log file we are writing to. The
// motivating use-case is the self-test subtests, each of which uses Isolates
// which log to different subdirectories.  While the sub-test is running, we'd
// like the parent self-test process to write to a file in the same directory.
// Modifying the log location requires a global (since logging is handled
// globally), and has to be implemented by working with the `tracing_subscriber`
// crate.

/// Global function to change the current log file (if we are logging to a file).
pub fn change_log_file(file: &Path) -> std::result::Result<(), LoggingError> {
    if let Some(mut lfh) = log_file_holder() {
        lfh.change_log_file(file)?;
    }
    Ok(())
}

/// Global function to change the reset the current log file back to the
/// original (if we are logging to a file).
pub fn reset_log_file() -> std::result::Result<(), LoggingError> {
    if let Some(mut lfh) = log_file_holder() {
        lfh.reset_log_file()?;
    }
    Ok(())
}

// The singleton object that will track the information we need. Along with
// providing the implementation of the global functions, it can return a
// resettable Write object that can be stored in the tracing_subscriber's file
// layer.
#[derive(Debug)]
struct LogFileHolder {
    orig_path: PathBuf,
    writer: Arc<RwLock<File>>,
}

impl LogFileHolder {
    fn new(path: PathBuf, f: File) -> Self {
        Self { orig_path: path, writer: Arc::new(RwLock::new(f)) }
    }

    fn get_resettable_writer(&self) -> ResettableWriter {
        ResettableWriter::new(self.writer.clone())
    }

    fn change_log_file(&mut self, path: &Path) -> std::result::Result<(), LoggingError> {
        let file = open_log_file(path)?;
        let mut w = self.writer.write().unwrap_or_else(|e| e.into_inner());
        // Replace the writer.  The underlying File will be flushed and closed.
        *w = file;
        Ok(())
    }

    fn reset_log_file(&mut self) -> std::result::Result<(), LoggingError> {
        self.change_log_file(&self.orig_path.clone())?;
        Ok(())
    }
}

static LOG_FILE_HOLDER: OnceLock<Mutex<LogFileHolder>> = OnceLock::new();

// Initializer for the singleton
fn init_log_file_holder(p: PathBuf, f: File) {
    let lfh = LogFileHolder::new(p, f);
    LOG_FILE_HOLDER.set(Mutex::new(lfh)).expect("init_log_file_holder(): OnceLock::set() failed");
}

// Getter for the singleton.  If we were never initialized (e.g. because the logger
// was told not to log to a file), this will return None.
fn log_file_holder() -> Option<MutexGuard<'static, LogFileHolder>> {
    LOG_FILE_HOLDER.get().map(|mx| mx.lock().unwrap_or_else(|e| e.into_inner()))
}

// We need an object whose underlying file we can change, while meeting the
// needs of tracing_subscriber, which wants to own a Writer with a static
// lifetime.  These requirements lead to a File held by a RwLock (because
// the global object can be accessed from multiple threads), inside an Arc
// (because both the ResettableWriter and the LogFileHolder need access). We
// can't replace Arc with a reference, because of the requirement for a static
// lifetime.
struct ResettableWriter {
    w: Arc<RwLock<File>>,
}

impl ResettableWriter {
    fn new(w: Arc<RwLock<File>>) -> Self {
        Self { w }
    }
}

impl Write for ResettableWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut w = self.w.write().unwrap_or_else(|e| e.into_inner());
        Ok(w.write(buf)?)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut w = self.w.write().unwrap_or_else(|e| e.into_inner());
        Ok(w.flush()?)
    }
}

fn open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options
}

fn rotate_file(
    log_rotate_size: Option<u64>,
    log_rotations: u64,
    log_path: &PathBuf,
) -> Result<Option<File>, LoggingError> {
    let mut rot_path = log_path.clone();

    if let Some(log_rotate_size) = log_rotate_size {
        // log.rotate_size was set. We only rotate if the current file is bigger than that size,
        // so open the current file and, if it's smaller than that size, return it.
        match open_options().write(true).append(true).create(false).open(log_path) {
            Ok(mut f) => {
                if f.seek(SeekFrom::End(0)).map_err(LoggingError::CheckSize)? < log_rotate_size {
                    return Ok(Some(f));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            Err(e) => {
                return Err(LoggingError::OpenLog(e));
            }
        }
    }

    rot_path.set_file_name(format!("{}.{}", log_path.display(), log_rotations - 1));
    match remove_file(&rot_path) {
        Err(e) if e.kind() == ErrorKind::NotFound => (),
        other => other.map_err(LoggingError::DeleteLog)?,
    }

    for rotation in (0..log_rotations - 1).rev() {
        let prev_path = rot_path.clone();
        rot_path.set_file_name(format!("{}.{}", log_path.display(), rotation));
        match rename(&rot_path, prev_path) {
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            other => other.map_err(LoggingError::RotateLog)?,
        }
    }

    if let Some(log_rotate_size) = log_rotate_size {
        // When we move the most recent log into rotation, truncate it if it is larger than the
        // rotation length.
        match open_options().read(true).create(false).open(log_path) {
            Ok(mut f) => {
                let size = f.seek(SeekFrom::End(0)).map_err(LoggingError::CheckSize)?;
                let log_rotate_size = std::cmp::min(size, log_rotate_size);
                f.seek(SeekFrom::End(-(log_rotate_size as i64))).map_err(LoggingError::SeekLog)?;
                let mut new = open_options()
                    .write(true)
                    .create(true)
                    .open(&rot_path)
                    .map_err(LoggingError::OpenLog)?;
                new.write_all(b"<truncated for length>").map_err(LoggingError::TruncateNotice)?;
                let mut buf = [0; 4096];
                loop {
                    let got = f.read(&mut buf).map_err(LoggingError::ReadOldLog)?;
                    if got == 0 {
                        break;
                    }
                    new.write_all(&buf[..got]).map_err(LoggingError::WriteTruncatedLog)?;
                }
                match remove_file(&log_path) {
                    Err(e) if e.kind() == ErrorKind::NotFound => (),
                    other => other.map_err(LoggingError::DeleteLog)?,
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            Err(e) => {
                return Err(LoggingError::OpenLog(e));
            }
        }
    } else {
        match rename(&log_path, rot_path) {
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            other => other.map_err(LoggingError::RotateLog)?,
        }
    }
    Ok(None)
}

fn init_global_log_file(
    ctx: &EnvironmentContext,
    name: &PathBuf,
    log_dir_handling: LogDirHandling,
) -> Result<(), LoggingError> {
    let (f, log_path) = log_file_with_info(ctx, name, log_dir_handling)?;
    init_log_file_holder(log_path, f);
    Ok(())
}

pub fn log_file_with_info(
    ctx: &EnvironmentContext,
    name: &PathBuf,
    log_dir_handling: LogDirHandling,
) -> Result<(File, PathBuf), LoggingError> {
    let mut log_path: PathBuf = ctx.get(LOG_DIR).map_err(LoggingError::Config)?;
    create_dir_all(&log_path)?;
    log_path.push(name);

    let mut f: Option<File> = None;
    if let LogDirHandling::WithDirWithRotate = log_dir_handling {
        let log_rotations: Option<u64> = ctx.get(LOG_ROTATIONS).map_err(LoggingError::Config)?;
        let log_rotations = log_rotations.unwrap_or(0);
        if log_rotations > 0 {
            let log_rotate_size: Option<u64> =
                ctx.get(LOG_ROTATE_SIZE).map_err(LoggingError::Config)?;
            // rotate_file() returns Some(f) if it uses an existing file
            f = rotate_file(log_rotate_size, log_rotations, &log_path)?;
        }
    }

    if f.is_none() {
        f = Some(open_log_file(&log_path)?);
    }

    let f = f.unwrap();
    Ok((f, log_path))
}

pub fn log_file(
    ctx: &EnvironmentContext,
    name: &PathBuf,
    log_dir_handling: LogDirHandling,
) -> Result<File, LoggingError> {
    let (f, _) = log_file_with_info(ctx, name, log_dir_handling)?;
    Ok(f)
}

fn open_log_file(path: &Path) -> std::result::Result<std::fs::File, LoggingError> {
    open_options().write(true).append(true).create(true).open(path).map_err(LoggingError::OpenLog)
}

pub fn is_enabled(ctx: &EnvironmentContext) -> bool {
    ctx.get(LOG_ENABLED).unwrap_or(false)
}

pub fn debugging_on(ctx: &EnvironmentContext) -> bool {
    let level = filter_level(ctx);
    level >= LevelFilter::Debug
}

fn filter_level(ctx: &EnvironmentContext) -> LevelFilter {
    ctx
        .get::<String,_>(LOG_LEVEL)
        .ok()
        .map(|str|
            // Ideally we could log here, but there may be no log sink, so print a warning to
            // stderr and fall back to a 'sensible' default
            LevelFilter::from_str(&str).unwrap_or_else(|_| {
                eprintln!("Warning: '{str}' is not a valid log level.\n\
                    \n\
                    Supported log levels are 'Off', 'Error', 'Warn', 'Info', 'Debug', and 'Trace'.\n\
                    \n\
                    If you didn't pass this log level with `--log-level`, you may need to change your \
                    configured log level to something valid with `ffx config set log.level`");
                LevelFilter::Info
            })
        )
        .unwrap_or(LevelFilter::Info)
}

pub fn init(
    ctx: &EnvironmentContext,
    mut log_to_stdio: bool,
    log_destination: &Option<LogDestination>,
) -> Result<(), LoggingError> {
    let mut destinations = vec![];
    let mut log_file_err = None;

    // We log to a file if config(log.enabled) is true, AND if either of the following are true:
    // * no destination is specified (in which case we log to <log.dir>/<LOG_PREFIX>.log
    // * log_destination is File(path)
    // Otherwise we only log to stdio
    // Logging can't be completely disabled, but the user can specify the destination as /dev/null
    if is_enabled(ctx) {
        match log_destination {
            None => {
                match init_global_log_file(
                    ctx,
                    &PathBuf::from(LOG_FILENAME),
                    LogDirHandling::WithDirWithRotate,
                ) {
                    Ok(()) => destinations.push(LogDestination::Global),
                    Err(e) => log_file_err = Some(e),
                }
            }
            Some(f @ LogDestination::File(_)) => {
                destinations.push(f.clone());
            }
            Some(LogDestination::Stdout) => {
                // Stdout gets special handling, since "-v" results in logs going
                // to _both_ stdout and the destination. But we don't want to
                // log twice if `-v --log-output -` is specified.
                log_to_stdio = true;
            }
            Some(d) => {
                destinations.push(d.clone());
            }
        }
    }

    if log_to_stdio {
        destinations.push(LogDestination::Stdout);
    }

    setup_logging_with_log(ctx, destinations)?;

    if let Some(e) = log_file_err {
        log::debug!("Could not initialize global log file: {e}");
    }

    log::info!("ffx logging initialized. ffx version info: {:?}", ffx_build_version::build_info());

    Ok(())
}

pub struct DisableableFilter;

fn level_filter(ctx: &EnvironmentContext) -> log::LevelFilter {
    ctx
        .get::<String, _>(LOG_LEVEL)
        .ok()
        .map(|str|
            // Ideally we could log here, but there may be no log sink, so print a warning to
            // stderr and fall back to a 'sensible' default
            log::LevelFilter::from_str(&str).unwrap_or_else(|_| {
                eprintln!("Warning: '{str}' is not a valid log level.\n\
                    \n\
                    Supported log levels are 'Off', 'Error', 'Warn', 'Info', 'Debug', and 'Trace'.\n\
                    \n\
                    If you didn't pass this log level with `--log-level`, you may need to change your \
                    configured log level to something valid with `ffx config set log.level`");
                log::LevelFilter::Info
            })
        )
        .unwrap_or(log::LevelFilter::Info)
}
pub fn target_levels_log(ctx: &EnvironmentContext) -> Vec<(String, log::LevelFilter)> {
    // Parse the targets from the config. Ideally we'd log errors, but since there might be no log
    // sink, filter out any unexpected values.
    if let Ok(targets) = ctx.get::<serde_json::Value, _>(LOG_TARGET_LEVELS) {
        if let serde_json::Value::Object(o) = targets {
            return o
                .into_iter()
                .filter_map(|(target, level)| {
                    if let serde_json::Value::String(level) = level {
                        if let Ok(level) = log::LevelFilter::from_str(&level) {
                            return Some((target, level));
                        }
                    }
                    None
                })
                .collect();
        }
    }

    vec![]
}

pub fn setup_logging_with_log(
    ctx: &EnvironmentContext,
    destinations: Vec<LogDestination>,
) -> Result<(), LoggingError> {
    let logger = build_logger_with_destinations(ctx, destinations)?;
    let _ = log::set_boxed_logger(Box::new(logger))
        .map(|()| log::set_max_level(log::LevelFilter::Trace));
    Ok(())
}

pub fn build_logger_with_destinations(
    ctx: &EnvironmentContext,
    destinations: Vec<LogDestination>,
) -> Result<impl log::Log + use<>, LoggingError> {
    let level = level_filter(ctx);
    let target_levels = target_levels_log(ctx);
    let format = FormatOpts::new(*LOGGING_ID);

    let toggle_filter = DisableableFilter {};
    let targets_filter = TargetsFilter::new(target_levels);

    let mut sinks: Vec<Box<dyn LogSinkTrait>> = vec![];

    if destinations.contains(&LogDestination::Stderr) {
        sinks.push(FfxLogSink::new(Arc::new(Mutex::new(std::io::stderr()))).boxed());
    }
    if destinations.contains(&LogDestination::Stdout) {
        sinks.push(FfxLogSink::new(Arc::new(Mutex::new(std::io::stdout()))).boxed());
    }
    if destinations.contains(&LogDestination::TestWriter) {
        sinks.push(FfxLogSink::new(Arc::new(Mutex::new(TestWriter::default()))).boxed());
    }
    if destinations.contains(&LogDestination::Global) {
        let lfh = log_file_holder().expect("uninitialized LFH when use_file is set??");
        sinks.push(
            FfxLogSink::new(Arc::new(Mutex::new(std::io::LineWriter::new(
                lfh.get_resettable_writer(),
            ))))
            .boxed(),
        );
    }
    for d in destinations {
        if let LogDestination::File(p) = d {
            let fres = open_log_file(p.as_path());
            match fres {
                Ok(f) => {
                    sinks.push(FfxLogSink::new(Arc::new(Mutex::new(f))).boxed());
                }
                Err(e) => {
                    eprintln!("Could not log file: {p:?}: {e}");
                }
            }
        }
    }

    Ok(FfxLog::new(sinks, format, toggle_filter, level, targets_filter))
}

impl Filter for DisableableFilter {
    fn should_emit(&self, _record: &log::Metadata<'_>) -> bool {
        LOG_ENABLED_FLAG.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_open_log_file_creates_and_appends() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("test.log");

        {
            let mut f =
                open_log_file(&log_path).expect("open_log_file should succeed for regular path");
            f.write_all(b"hello ").unwrap();
        }

        {
            let mut f =
                open_log_file(&log_path).expect("open_log_file should succeed when appending");
            f.write_all(b"world").unwrap();
        }

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[cfg(unix)]
    #[test]
    fn test_open_log_file_rejects_symlink() {
        let dir = tempdir().unwrap();
        let target_path = dir.path().join("sensitive.txt");
        std::fs::write(&target_path, "sensitive data\n").unwrap();

        let symlink_path = dir.path().join("symlink.log");
        std::os::unix::fs::symlink(&target_path, &symlink_path).unwrap();

        let res = open_log_file(&symlink_path);
        assert!(res.is_err(), "open_log_file must reject symlinks");

        // Verify target file content was not modified
        let target_content = std::fs::read_to_string(&target_path).unwrap();
        assert_eq!(target_content, "sensitive data\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_rotate_file_rejects_symlink() {
        let dir = tempdir().unwrap();
        let target_path = dir.path().join("sensitive_rotate.txt");
        std::fs::write(&target_path, "sensitive rotate data\n").unwrap();

        let symlink_path = dir.path().join("symlink_rotate.log");
        std::os::unix::fs::symlink(&target_path, &symlink_path).unwrap();

        let res = rotate_file(Some(100), 2, &symlink_path);
        assert!(res.is_err(), "rotate_file must reject symlinks when checking size");

        // Verify target file content was not modified
        let target_content = std::fs::read_to_string(&target_path).unwrap();
        assert_eq!(target_content, "sensitive rotate data\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_log_file_with_info_rejects_symlink() {
        let dir = tempdir().unwrap();
        let target_path = dir.path().join("sensitive_target.txt");
        std::fs::write(&target_path, "sensitive info\n").unwrap();

        let log_dir = dir.path().join("logs");
        std::fs::create_dir(&log_dir).unwrap();
        let symlink_path = log_dir.join(LOG_FILENAME);
        std::os::unix::fs::symlink(&target_path, &symlink_path).unwrap();

        let test_env = crate::test_env()
            .user_config(LOG_DIR, log_dir.to_str().unwrap())
            .build()
            .expect("create test config");

        let res = log_file_with_info(
            &test_env.context,
            &PathBuf::from(LOG_FILENAME),
            LogDirHandling::WithDirWithoutRotate,
        );
        assert!(res.is_err(), "log_file_with_info must reject opening symlinks");

        let target_content = std::fs::read_to_string(&target_path).unwrap();
        assert_eq!(target_content, "sensitive info\n");
    }

    #[test]
    fn test_init_succeeds_with_unresolvable_log_dir() {
        let ctx = EnvironmentContext::no_context(
            crate::environment::ExecutableKind::Test,
            Default::default(),
            None,
            true,
        )
        .unwrap();

        // When log_destination is None and log.dir is unresolvable, init() should succeed gracefully.
        let res = init(&ctx, false, &None);
        assert!(res.is_ok(), "init should succeed even if default log file cannot be initialized");
    }
}
