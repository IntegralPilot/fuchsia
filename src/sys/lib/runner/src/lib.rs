// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod component;
pub mod serde;

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_runner as fcrunner;
#[cfg(fuchsia_api_level_at_least = "HEAD")]
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_component_sandbox as _;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_process as fprocess;
use std::path::Path;
use thiserror::Error;

const ARGS_KEY: &str = "args";
const BINARY_KEY: &str = "binary";
const ENVIRON_KEY: &str = "environ";

/// An error encountered trying to get entry out of `ComponentStartInfo->program`.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StartInfoProgramError {
    #[error("\"program.binary\" must be specified")]
    MissingBinary,

    #[error("the value of \"program.binary\" must be a string")]
    InValidBinaryType,

    #[error("the value of \"program.binary\" must be a relative path")]
    BinaryPathNotRelative,

    #[error("the value of \"program.{0}\" must be an array of strings")]
    InvalidStrVec(String),

    #[error("\"program\" must be specified")]
    NotFound,

    #[error("invalid type for key \"{0}\", expected string")]
    InvalidType(String),

    #[error("invalid value for key \"{0}\", expected one of \"{1}\", found \"{2}\"")]
    InvalidValue(String, String, String),

    #[error("environ value at index \"{0}\" is invalid. Value must be format of 'VARIABLE=VALUE'")]
    InvalidEnvironValue(usize),
}

/// Retrieves component URL from start_info or errors out if not found.
pub fn get_resolved_url(start_info: &fcrunner::ComponentStartInfo) -> Option<String> {
    start_info.resolved_url.clone()
}

/// Returns a reference to the value corresponding to the key.
pub fn get_value<'a>(dict: &'a fdata::Dictionary, key: &str) -> Option<&'a fdata::DictionaryValue> {
    match &dict.entries {
        Some(entries) => {
            for entry in entries {
                if entry.key == key {
                    return entry.value.as_ref().map(|val| &**val);
                }
            }
            None
        }
        _ => None,
    }
}

/// Retrieve a reference to the enum value corresponding to the key.
pub fn get_enum<'a>(
    dict: &'a fdata::Dictionary,
    key: &str,
    variants: &[&str],
) -> Result<Option<&'a str>, StartInfoProgramError> {
    match get_value(dict, key) {
        Some(fdata::DictionaryValue::Str(value)) => {
            if variants.contains(&value.as_str()) {
                Ok(Some(value.as_ref()))
            } else {
                Err(StartInfoProgramError::InvalidValue(
                    key.to_owned(),
                    format!("{:?}", variants),
                    value.to_owned(),
                ))
            }
        }
        Some(_) => Err(StartInfoProgramError::InvalidType(key.to_owned())),
        None => Ok(None),
    }
}

/// Retrieve value of type bool. Defaults to 'false' if key is not found.
pub fn get_bool<'a>(dict: &'a fdata::Dictionary, key: &str) -> Result<bool, StartInfoProgramError> {
    match get_enum(dict, key, &["true", "false"])? {
        Some("true") => Ok(true),
        _ => Ok(false),
    }
}

/// Retrieve value of type string, or None if not present.
pub fn get_string<'a>(dict: &'a fdata::Dictionary, key: &str) -> Option<&'a str> {
    if let fdata::DictionaryValue::Str(value) = get_value(dict, key)? { Some(value) } else { None }
}

fn get_program_value<'a>(
    start_info: &'a fcrunner::ComponentStartInfo,
    key: &str,
) -> Option<&'a fdata::DictionaryValue> {
    get_value(start_info.program.as_ref()?, key)
}

/// Retrieve a string from the program dictionary in ComponentStartInfo.
pub fn get_program_string<'a>(
    start_info: &'a fcrunner::ComponentStartInfo,
    key: &str,
) -> Option<&'a str> {
    if let fdata::DictionaryValue::Str(value) = get_program_value(start_info, key)? {
        Some(value)
    } else {
        None
    }
}

/// Retrieve a StrVec from the program dictionary in ComponentStartInfo. Returns StartInfoProgramError::InvalidStrVec if
/// the value is not a StrVec.
pub fn get_program_strvec<'a>(
    start_info: &'a fcrunner::ComponentStartInfo,
    key: &str,
) -> Result<Option<&'a Vec<String>>, StartInfoProgramError> {
    match get_program_value(start_info, key) {
        Some(args_value) => match args_value {
            fdata::DictionaryValue::StrVec(vec) => Ok(Some(vec)),
            _ => Err(StartInfoProgramError::InvalidStrVec(key.to_string())),
        },
        None => Ok(None),
    }
}

/// Retrieves program.binary from ComponentStartInfo and makes sure that path is relative.
// TODO(https://fxbug.dev/42079981): This method should accept a program dict instead of start_info
pub fn get_program_binary(
    start_info: &fcrunner::ComponentStartInfo,
) -> Result<String, StartInfoProgramError> {
    if let Some(program) = &start_info.program {
        get_program_binary_from_dict(&program)
    } else {
        Err(StartInfoProgramError::NotFound)
    }
}

/// Retrieves `binary` from a ComponentStartInfo dict and makes sure that path is relative.
pub fn get_program_binary_from_dict(
    dict: &fdata::Dictionary,
) -> Result<String, StartInfoProgramError> {
    if let Some(val) = get_value(&dict, BINARY_KEY) {
        if let fdata::DictionaryValue::Str(bin) = val {
            if !Path::new(bin).is_absolute() {
                Ok(bin.to_string())
            } else {
                Err(StartInfoProgramError::BinaryPathNotRelative)
            }
        } else {
            Err(StartInfoProgramError::InValidBinaryType)
        }
    } else {
        Err(StartInfoProgramError::MissingBinary)
    }
}

/// Retrieves program.args from ComponentStartInfo and validates them.
// TODO(https://fxbug.dev/42079981): This method should accept a program dict instead of start_info
pub fn get_program_args(
    start_info: &fcrunner::ComponentStartInfo,
) -> Result<Vec<String>, StartInfoProgramError> {
    match get_program_strvec(start_info, ARGS_KEY)? {
        Some(vec) => Ok(vec.iter().map(|v| v.clone()).collect()),
        None => Ok(vec![]),
    }
}

/// Retrieves `args` from a ComponentStartInfo program dict and validates them.
pub fn get_program_args_from_dict(
    dict: &fdata::Dictionary,
) -> Result<Vec<String>, StartInfoProgramError> {
    match get_value(&dict, ARGS_KEY) {
        Some(args_value) => match args_value {
            fdata::DictionaryValue::StrVec(vec) => Ok(vec.iter().map(|v| v.clone()).collect()),
            _ => Err(StartInfoProgramError::InvalidStrVec(ARGS_KEY.to_string())),
        },
        None => Ok(vec![]),
    }
}

pub fn get_environ(dict: &fdata::Dictionary) -> Result<Option<Vec<String>>, StartInfoProgramError> {
    match get_value(dict, ENVIRON_KEY) {
        Some(fdata::DictionaryValue::StrVec(values)) => {
            if values.is_empty() {
                return Ok(None);
            }
            for (i, value) in values.iter().enumerate() {
                let parts = value.split_once("=");
                if parts.is_none() {
                    return Err(StartInfoProgramError::InvalidEnvironValue(i));
                }
                let parts = parts.unwrap();
                // The value of an environment variable can in fact be empty.
                if parts.0.is_empty() {
                    return Err(StartInfoProgramError::InvalidEnvironValue(i));
                }
            }
            Ok(Some(values.clone()))
        }
        Some(fdata::DictionaryValue::Str(_)) => Err(StartInfoProgramError::InvalidValue(
            ENVIRON_KEY.to_owned(),
            "vector of string".to_owned(),
            "string".to_owned(),
        )),
        Some(other) => Err(StartInfoProgramError::InvalidValue(
            ENVIRON_KEY.to_owned(),
            "vector of string".to_owned(),
            format!("{:?}", other),
        )),
        None => Ok(None),
    }
}

/// Errors from parsing a component's configuration data.
#[derive(Debug, Clone, Error)]
pub enum ConfigDataError {
    #[error("failed to create a vmo: {_0}")]
    VmoCreate(#[source] zx::Status),
    #[error("failed to write to vmo: {_0}")]
    VmoWrite(#[source] zx::Status),
    #[error("encountered an unrecognized variant of fuchsia.mem.Data")]
    UnrecognizedDataVariant,
}

pub fn get_config_vmo(encoded_config: fmem::Data) -> Result<zx::Vmo, ConfigDataError> {
    match encoded_config {
        fmem::Data::Buffer(fmem::Buffer {
            vmo,
            size: _, // we get this vmo from component manager which sets the content size
        }) => Ok(vmo),
        fmem::Data::Bytes(bytes) => {
            let size = bytes.len() as u64;
            let vmo = zx::Vmo::create(size).map_err(ConfigDataError::VmoCreate)?;
            vmo.write(&bytes, 0).map_err(ConfigDataError::VmoWrite)?;
            Ok(vmo)
        }
        _ => Err(ConfigDataError::UnrecognizedDataVariant.into()),
    }
}

/// Errors from parsing ComponentStartInfo.
#[derive(Debug, Clone, Error)]
pub enum StartInfoError {
    #[error("missing program")]
    MissingProgram,
    #[error("missing resolved URL")]
    MissingResolvedUrl,
}

impl StartInfoError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            StartInfoError::MissingProgram => zx::Status::INVALID_ARGS,
            StartInfoError::MissingResolvedUrl => zx::Status::INVALID_ARGS,
        }
    }
}

/// [StartInfo] is convertible from the FIDL [fcrunner::ComponentStartInfo]
/// type and performs validation that makes sense for all runners in the process.
pub struct StartInfo {
    /// The resolved URL of the component.
    ///
    /// This is the canonical URL obtained by the component resolver after
    /// following redirects and resolving relative paths.
    pub resolved_url: String,

    /// The component's program declaration.
    /// This information originates from `ComponentDecl.program`.
    pub program: fdata::Dictionary,

    /// The namespace to provide to the component instance.
    ///
    /// A namespace specifies the set of directories that a component instance
    /// receives at start-up. Through the namespace directories, a component
    /// may access capabilities available to it. The contents of the namespace
    /// are mainly determined by the component's `use` declarations but may
    /// also contain additional capabilities automatically provided by the
    /// framework.
    ///
    /// By convention, a component's namespace typically contains some or all
    /// of the following directories:
    ///
    /// - "/svc": A directory containing services that the component requested
    ///           to use via its "import" declarations.
    /// - "/pkg": A directory containing the component's package, including its
    ///           binaries, libraries, and other assets.
    ///
    /// The mount points specified in each entry must be unique and
    /// non-overlapping. For example, [{"/foo", ..}, {"/foo/bar", ..}] is
    /// invalid.
    pub namespace: Vec<fcrunner::ComponentNamespaceEntry>,

    /// The directory this component serves.
    pub outgoing_dir: Option<ServerEnd<fio::DirectoryMarker>>,

    /// The directory served by the runner to present runtime information about
    /// the component. The runner must either serve it, or drop it to avoid
    /// blocking any consumers indefinitely.
    pub runtime_dir: Option<ServerEnd<fio::DirectoryMarker>>,

    /// The numbered handles that were passed to the component.
    ///
    /// If the component does not support numbered handles, the runner is expected
    /// to close the handles.
    pub numbered_handles: Vec<fprocess::HandleInfo>,

    /// Binary representation of the component's configuration.
    ///
    /// # Layout
    ///
    /// The first 2 bytes of the data should be interpreted as an unsigned 16-bit
    /// little-endian integer which denotes the number of bytes following it that
    /// contain the configuration checksum. After the checksum, all the remaining
    /// bytes are a persistent FIDL message of a top-level struct. The struct's
    /// fields match the configuration fields of the component's compiled manifest
    /// in the same order.
    pub encoded_config: Option<fmem::Data>,

    /// An eventpair that debuggers can use to defer the launch of the component.
    ///
    /// For example, ELF runners hold off from creating processes in the component
    /// until ZX_EVENTPAIR_PEER_CLOSED is signaled on this eventpair. They also
    /// ensure that runtime_dir is served before waiting on this eventpair.
    /// ELF debuggers can query the runtime_dir to decide whether to attach before
    /// they drop the other side of the eventpair, which is sent in the payload of
    /// the DebugStarted event in fuchsia.component.events.
    pub break_on_start: Option<zx::EventPair>,

    /// An opaque token that represents the component instance.
    ///
    /// The `fuchsia.component/Introspector` protocol may be used to get the
    /// string moniker of the instance from this token.
    ///
    /// Runners may publish this token as part of diagnostics information, to
    /// identify the running component without knowing its moniker.
    ///
    /// The token is invalidated when the component instance is destroyed.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub component_instance: Option<zx::Event>,

    /// A dictionary containing data and handles that the component has escrowed
    /// during its previous execution via
    /// `fuchsia.component.runner/ComponentController.OnEscrow`.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub escrowed_dictionary: Option<fsandbox::DictionaryRef>,

    /// A dictionary containing data and handles that the component has escrowed
    /// during its previous execution via
    /// `fuchsia.component.runner/ComponentController.OnEscrow`.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub escrowed_dictionary_handle: Option<zx::EventPair>,
}

impl TryFrom<fcrunner::ComponentStartInfo> for StartInfo {
    type Error = StartInfoError;
    fn try_from(start_info: fcrunner::ComponentStartInfo) -> Result<Self, Self::Error> {
        let resolved_url = start_info.resolved_url.ok_or(StartInfoError::MissingResolvedUrl)?;
        let program = start_info.program.ok_or(StartInfoError::MissingProgram)?;
        Ok(Self {
            resolved_url,
            program,
            namespace: start_info.ns.unwrap_or_else(|| Vec::new()),
            outgoing_dir: start_info.outgoing_dir,
            runtime_dir: start_info.runtime_dir,
            numbered_handles: start_info.numbered_handles.unwrap_or_else(|| Vec::new()),
            encoded_config: start_info.encoded_config,
            break_on_start: start_info.break_on_start,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            component_instance: start_info.component_instance,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            escrowed_dictionary: start_info.escrowed_dictionary,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            escrowed_dictionary_handle: start_info.escrowed_dictionary_handle,
        })
    }
}

impl From<StartInfo> for fcrunner::ComponentStartInfo {
    fn from(start_info: StartInfo) -> Self {
        Self {
            resolved_url: Some(start_info.resolved_url),
            program: Some(start_info.program),
            ns: Some(start_info.namespace),
            outgoing_dir: start_info.outgoing_dir,
            runtime_dir: start_info.runtime_dir,
            numbered_handles: Some(start_info.numbered_handles),
            encoded_config: start_info.encoded_config,
            break_on_start: start_info.break_on_start,

            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            component_instance: start_info.component_instance,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            escrowed_dictionary: start_info.escrowed_dictionary,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            escrowed_dictionary_handle: start_info.escrowed_dictionary_handle,

            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(Some("some_url"), Some("some_url".to_owned()) ; "when url is valid")]
    #[test_case(None, None ; "when url is missing")]
    fn get_resolved_url_test(maybe_url: Option<&str>, expected: Option<String>) {
        let start_info = fcrunner::ComponentStartInfo {
            resolved_url: maybe_url.map(str::to_owned),
            program: None,
            ns: None,
            outgoing_dir: None,
            runtime_dir: None,
            ..Default::default()
        };
        assert_eq!(get_resolved_url(&start_info), expected,);
    }

    #[test_case(Some("bin/myexecutable"), Ok("bin/myexecutable".to_owned()) ; "when binary value is valid")]
    #[test_case(Some("/bin/myexecutable"), Err(StartInfoProgramError::BinaryPathNotRelative) ; "when binary path is not relative")]
    #[test_case(None, Err(StartInfoProgramError::NotFound) ; "when program stanza is not set")]
    fn get_program_binary_test(
        maybe_value: Option<&str>,
        expected: Result<String, StartInfoProgramError>,
    ) {
        let start_info = match maybe_value {
            Some(value) => new_start_info(Some(new_program_stanza("binary", value))),
            None => new_start_info(None),
        };
        assert_eq!(get_program_binary(&start_info), expected);
    }

    #[test]
    fn get_program_binary_test_when_binary_key_is_missing() {
        let start_info = new_start_info(Some(new_program_stanza("some_other_key", "bin/foo")));
        assert_eq!(get_program_binary(&start_info), Err(StartInfoProgramError::MissingBinary));
    }

    #[test_case("bin/myexecutable", Ok("bin/myexecutable".to_owned()) ; "when binary value is valid")]
    #[test_case("/bin/myexecutable", Err(StartInfoProgramError::BinaryPathNotRelative) ; "when binary path is not relative")]
    fn get_program_binary_from_dict_test(
        value: &str,
        expected: Result<String, StartInfoProgramError>,
    ) {
        let program = new_program_stanza("binary", value);
        assert_eq!(get_program_binary_from_dict(&program), expected);
    }

    #[test]
    fn get_program_binary_from_dict_test_when_binary_key_is_missing() {
        let program = new_program_stanza("some_other_key", "bin/foo");
        assert_eq!(
            get_program_binary_from_dict(&program),
            Err(StartInfoProgramError::MissingBinary)
        );
    }

    #[test_case(&[], vec![] ; "when args is empty")]
    #[test_case(&["a".to_owned()], vec!["a".to_owned()] ; "when args is a")]
    #[test_case(&["a".to_owned(), "b".to_owned()], vec!["a".to_owned(), "b".to_owned()] ; "when args a and b")]
    fn get_program_args_test(args: &[String], expected: Vec<String>) {
        let start_info =
            new_start_info(Some(new_program_stanza_with_vec(ARGS_KEY, Vec::from(args))));
        assert_eq!(get_program_args(&start_info).unwrap(), expected);
    }

    #[test_case(&[], vec![] ; "when args is empty")]
    #[test_case(&["a".to_owned()], vec!["a".to_owned()] ; "when args is a")]
    #[test_case(&["a".to_owned(), "b".to_owned()], vec!["a".to_owned(), "b".to_owned()] ; "when args a and b")]
    fn get_program_args_from_dict_test(args: &[String], expected: Vec<String>) {
        let program = new_program_stanza_with_vec(ARGS_KEY, Vec::from(args));
        assert_eq!(get_program_args_from_dict(&program).unwrap(), expected);
    }

    #[test]
    fn get_program_args_invalid() {
        let program = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: ARGS_KEY.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("hello".to_string()))),
            }]),
            ..Default::default()
        };
        assert_eq!(
            get_program_args_from_dict(&program),
            Err(StartInfoProgramError::InvalidStrVec(ARGS_KEY.to_string()))
        );
    }

    #[test_case(fdata::DictionaryValue::StrVec(vec!["foo=bar".to_owned(), "bar=baz".to_owned()]), Ok(Some(vec!["foo=bar".to_owned(), "bar=baz".to_owned()])); "when_values_are_valid")]
    #[test_case(fdata::DictionaryValue::StrVec(vec![]), Ok(None); "when_value_is_empty")]
    #[test_case(fdata::DictionaryValue::StrVec(vec!["=bad".to_owned()]), Err(StartInfoProgramError::InvalidEnvironValue(0)); "for_environ_with_empty_left_hand_side")]
    #[test_case(fdata::DictionaryValue::StrVec(vec!["good=".to_owned()]), Ok(Some(vec!["good=".to_owned()])); "for_environ_with_empty_right_hand_side")]
    #[test_case(fdata::DictionaryValue::StrVec(vec!["no_equal_sign".to_owned()]), Err(StartInfoProgramError::InvalidEnvironValue(0)); "for_environ_with_no_delimiter")]
    #[test_case(fdata::DictionaryValue::StrVec(vec!["foo=bar=baz".to_owned()]), Ok(Some(vec!["foo=bar=baz".to_owned()])); "for_environ_with_multiple_delimiters")]
    #[test_case(fdata::DictionaryValue::Str("foo=bar".to_owned()), Err(StartInfoProgramError::InvalidValue(ENVIRON_KEY.to_owned(), "vector of string".to_owned(), "string".to_owned())); "for_environ_as_invalid_type")]
    fn get_environ_test(
        value: fdata::DictionaryValue,
        expected: Result<Option<Vec<String>>, StartInfoProgramError>,
    ) {
        let program = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: ENVIRON_KEY.to_owned(),
                value: Some(Box::new(value)),
            }]),
            ..Default::default()
        };

        assert_eq!(get_environ(&program), expected);
    }

    fn new_start_info(program: Option<fdata::Dictionary>) -> fcrunner::ComponentStartInfo {
        fcrunner::ComponentStartInfo {
            program: program,
            ns: None,
            outgoing_dir: None,
            runtime_dir: None,
            resolved_url: None,
            ..Default::default()
        }
    }

    fn new_program_stanza(key: &str, value: &str) -> fdata::Dictionary {
        fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: key.to_owned(),
                value: Some(Box::new(fdata::DictionaryValue::Str(value.to_owned()))),
            }]),
            ..Default::default()
        }
    }

    fn new_program_stanza_with_vec(key: &str, values: Vec<String>) -> fdata::Dictionary {
        fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: key.to_owned(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(values))),
            }]),
            ..Default::default()
        }
    }
}
