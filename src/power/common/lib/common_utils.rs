// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Logs an error message if the passed in `result` is an error.
#[macro_export]
macro_rules! log_if_err {
    ($result:expr, $log_prefix:expr) => {
        if let Err(e) = $result.as_ref() {
            log::error!("{}: {}", $log_prefix, e);
        }
    };
}

pub mod result_debug_panic {
    /// Convenience wrapper to cause a debug panic if a Result is Err.
    ///
    /// Example:
    ///     let ok_val = result.or_debug_panic()?;
    ///
    /// In the example, the try operator is used to unwrap an Ok value or return if it is Err, while
    /// also debug panicking if it is Err.
    pub trait ResultDebugPanic<T, E> {
        fn or_debug_panic(self) -> Result<T, E>;
    }

    impl<T, E: std::fmt::Debug> ResultDebugPanic<T, E> for Result<T, E> {
        #[track_caller]
        fn or_debug_panic(self) -> Result<T, E> {
            #[cfg(debug_assertions)]
            if let Err(e) = self {
                panic!("Panicking in debug build because this should never happen; {:?}", e);
            }
            self
        }
    }

    #[cfg(test)]
    mod tests {
        use super::ResultDebugPanic;

        #[fuchsia::test]
        fn test_ok_result() {
            let res: Result<(), ()> = Ok(());
            assert_eq!(res.or_debug_panic(), Ok(()));
        }

        #[test] // Use instead of #[fuchsia::test] to suppress ERROR logs that cause test failure
        #[should_panic]
        #[cfg(debug_assertions)]
        fn test_err_debug() {
            let res: Result<(), ()> = Err(());
            let _ = res.or_debug_panic(); // panics
        }

        #[fuchsia::test]
        #[cfg(not(debug_assertions))]
        fn test_err_nondebug() {
            let res: Result<(), ()> = Err(());
            assert_eq!(res.or_debug_panic(), Err(()));
        }
    }
}

/// Exports a convenience macro, `ok_or_default_err`, which acts as a wrapper for `Option::ok_or` to
/// provide a default Err value. The macro transforms an Option<T> into a Result<T, E>, mapping
/// Some(v) to Ok(v) and None to Err("$var_name is None").
#[macro_use]
pub mod ok_or_default_err {
    #[macro_export]
    macro_rules! ok_or_default_err {
        ($result:expr) => {
            $result.ok_or(anyhow::format_err!("{} is None", stringify!($result)))
        };
    }

    #[cfg(test)]
    mod tests {
        #[fuchsia::test]
        fn test_some() {
            let foo = Some(1);
            assert_eq!(ok_or_default_err!(foo).unwrap(), 1);
        }

        #[fuchsia::test]
        fn test_none() {
            let foo: Option<()> = None;
            assert_eq!(ok_or_default_err!(foo).unwrap_err().to_string(), "foo is None")
        }
    }
}

/// The number of nanoseconds since the system was powered on.
pub fn get_current_timestamp() -> crate::types::Nanoseconds {
    crate::types::Nanoseconds(fuchsia_async::MonotonicInstant::now().into_nanos())
}

// Finds all of the node config files under the test package's "/config/data" directory. The node
// config files are identified by a suffix of "node_config.json5". The function then calls the
// provided `test_config_file` function for each found config file, passing the JSON structure in as
// an argument. The function returns success if each call to `test_config_file` succeeds. Otherwise,
// the first error encountered is returned.
#[cfg(test)]
pub fn test_each_node_config_file(
    test_config_file: impl Fn(&Vec<serde_json::Value>) -> Result<(), anyhow::Error>,
) -> Result<(), anyhow::Error> {
    use anyhow::Context as _;
    use std::fs;

    let suffix = "node_config.json5";
    let config_files = fs::read_dir("/pkg/config")
        .unwrap()
        .filter(|f| f.as_ref().unwrap().file_name().to_str().unwrap().ends_with(suffix))
        .map(|f| {
            let path = f.unwrap().path();
            let file_path = path.to_str().unwrap().to_string();
            let contents = std::fs::read_to_string(&file_path).unwrap();
            let json = serde_json5::from_str(&contents)
                .unwrap_or_else(|e| panic!("Failed to parse file {}: {:?}", file_path, e));

            (file_path, json)
        })
        .collect::<Vec<_>>();
    assert!(config_files.len() > 0, "No config files found");

    for (file_path, config_file) in config_files {
        test_config_file(&config_file).context(format!("Failed for file {}", file_path))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that the `get_current_timestamp` function returns the expected current timestamp.
    #[fuchsia::test]
    fn test_get_current_timestamp() {
        use crate::types::Nanoseconds;

        let exec = fuchsia_async::TestExecutor::new_with_fake_time();

        exec.set_fake_time(fuchsia_async::MonotonicInstant::from_nanos(0));
        assert_eq!(get_current_timestamp(), Nanoseconds(0));

        exec.set_fake_time(fuchsia_async::MonotonicInstant::from_nanos(1000));
        assert_eq!(get_current_timestamp(), Nanoseconds(1000));
    }
}
