// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#![deny(missing_docs)]

//! Provides the `log::Log` implementation that allows to publish `log` events to
//! Fuchsia Logging System.
//!
//! This library isn't Fuchsia-specific and provides a general `log::Log` that allows
//! the library to also be used in the host.

#[cfg(all(target_os = "fuchsia", fuchsia_api_level_at_least = "29"))]
mod fuchsia;
#[cfg(all(target_os = "fuchsia", fuchsia_api_level_at_least = "29"))]
use self::fuchsia as implementation;

// Version 28 of the SDK was the last version that used sockets for the Rust
// client. The latest uses IOBuffers.
#[cfg(all(target_os = "fuchsia", not(fuchsia_api_level_at_least = "29")))]
mod fuchsia_28;
#[cfg(all(target_os = "fuchsia", not(fuchsia_api_level_at_least = "29")))]
use self::fuchsia_28 as implementation;

#[cfg(not(target_os = "fuchsia"))]
mod portable;
#[cfg(not(target_os = "fuchsia"))]
use self::portable as implementation;

pub use diagnostics_log_types::Severity;
pub use implementation::*;

/// Adds a panic hook which will log an `ERROR` log with the panic information.
pub(crate) fn install_panic_hook(prefix: Option<&'static str>) {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = format!("{info}");
        let prefix = prefix.unwrap_or("PANIC");
        log::logger().log(
            &log::RecordBuilder::new()
                .level(log::Level::Error)
                .line(Some(info.location().unwrap().line()))
                .file(Some(info.location().unwrap().file()))
                .key_values(&[("info", message.as_str())])
                .args(format_args!("{prefix}"))
                .build(),
        );
        previous_hook(info);
    }));
}

/// A type which has a `Severity`.
pub trait SeverityExt {
    /// Return the severity of this value.
    fn severity(&self) -> Severity;

    /// Return the raw severity of this value.
    fn raw_severity(&self) -> u8 {
        self.severity() as u8
    }
}

impl SeverityExt for log::Metadata<'_> {
    fn severity(&self) -> Severity {
        match self.level() {
            log::Level::Error => Severity::Error,
            log::Level::Warn => Severity::Warn,
            log::Level::Info => Severity::Info,
            log::Level::Debug => Severity::Debug,
            log::Level::Trace => Severity::Trace,
        }
    }
}

/// Options to configure publishing. This is for initialization of logs, it's a superset of
/// `PublisherOptions`.
// NOTE: `Default` is implemented by the target specific implementations.
pub struct PublishOptions<'t> {
    pub(crate) publisher: PublisherOptions<'t>,
    pub(crate) install_panic_hook: bool,
    pub(crate) panic_prefix: Option<&'static str>,
}

impl PublishOptions<'_> {
    /// Whether or not to install a panic hook which will log an ERROR whenever a panic happens.
    ///
    /// Default: true.
    pub fn install_panic_hook(mut self, enable: bool) -> Self {
        self.install_panic_hook = enable;
        self
    }

    /// Enable to always log file/line information, otherwise only log
    /// when severity is ERROR or above.
    #[cfg(target_os = "fuchsia")]
    pub fn always_log_file_line(mut self) -> Self {
        self.publisher.always_log_file_line = true;
        self
    }

    /// Override the default string prefix for a logged panic message.
    pub fn panic_prefix(mut self, prefix: &'static str) -> Self {
        self.panic_prefix = Some(prefix);
        self
    }
}

macro_rules! publisher_options {
    ($(($name:ident, $self:ident, $($self_arg:ident),*)),*) => {
        $(
            impl<'t> $name<'t> {
                /// Sets the tags applied to all published events.
                ///
                /// When set to an empty slice (the default), events are tagged with the moniker of
                /// the component in which they are recorded.
                ///
                /// Default: empty.
                pub fn tags(mut $self, tags: &'t [&'t str]) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.tags = tags;
                    $self
                }

                /// Enable a metatag. It'll be applied to all published events.
                ///
                /// Default: no metatags are enabled.
                pub fn enable_metatag(mut $self, metatag: Metatag) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.metatags.insert(metatag);
                    $self
                }

                /// An interest filter to apply to messages published.
                ///
                /// Default: EMPTY, which implies INFO.
                pub fn minimum_severity(mut $self, severity: impl Into<Severity>) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.interest.min_severity = Some(severity.into().into());
                    $self
                }
            }
        )*
    };
}

publisher_options!((PublisherOptions, self,), (PublishOptions, self, publisher));
