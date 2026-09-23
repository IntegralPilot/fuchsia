// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod builder;
mod env;
mod errors;
mod logger;
mod name;
mod parsers;
mod run_tests;
mod schema;
mod std_writer;
mod test_config;

use crate::env::{ActualEnv, EnvLike};
use crate::logger::{RetainingLogger, StderrLogger};
use crate::schema::Schema;
use std::fs;
use test_pilot_lib::test_output::OutputDirectory;

const INVALID_ARGS_CONFIG_EXIT_CODE: i32 = 222;
const ENV_PATH: &str = "PATH";

// We need multiple threads to stream stdout/err in parallel until we can switch this to use tokio.
#[fuchsia::main(threads = 2)]
async fn main() {
    let env = ActualEnv;
    let schema = match Schema::from_env_like(&env) {
        Ok(schema) => schema,
        Err(error) => {
            eprintln!("{}", error);
            std::process::exit(INVALID_ARGS_CONFIG_EXIT_CODE);
        }
    };

    let test_config_result = if env.contains_debug_arg() {
        // Print log messages immediately regardless of outcome.
        test_config::TestConfig::from_env_like(&env, schema, &mut StderrLogger)
    } else {
        // Print log messages if there's an error.
        let mut logger = RetainingLogger::new();

        let result = test_config::TestConfig::from_env_like(&env, schema, &mut logger);

        if result.is_err() {
            eprintln!("BEGIN PARAMETER PROCESSING LOG FOR DEBUGGING");
            logger.eprintln();
            eprintln!("END PARAMETER PROCESSING LOG FOR DEBUGGING");
            eprintln!();
        }

        result
    };

    match test_config_result {
        Ok(test_config) => {
            let outdir = OutputDirectory::new(&test_config.output_directory);

            if let Err(e) = fs::create_dir_all(&test_config.output_directory) {
                eprintln!(
                    "Error creating output directory {}: {}",
                    test_config.output_directory.display(),
                    e
                );
                std::process::exit(INVALID_ARGS_CONFIG_EXIT_CODE);
            }

            if let Err(e) =
                fs::write(outdir.test_config(), serde_json::to_string_pretty(&test_config).unwrap())
            {
                eprintln!(
                    "Error writing test parameters to {}: {}",
                    outdir.test_config().display(),
                    e
                );
                std::process::exit(INVALID_ARGS_CONFIG_EXIT_CODE);
            }

            println!(
                "Valid args received: {}",
                serde_json::to_string_pretty(&test_config).unwrap()
            );

            // TODO(b/294567715): Map error to output format.
            match run_tests::run_test(
                &test_config,
                &outdir,
                std::env::var(ENV_PATH).unwrap().as_str(),
            )
            .await
            {
                Ok(exit_code) => std::process::exit(exit_code.code().unwrap()),
                Err(e) => {
                    eprintln!("Error running test: {}", e);
                    std::process::exit(INVALID_ARGS_CONFIG_EXIT_CODE);
                }
            }
        }
        Err(error) => {
            match error {
                errors::BuildError::ValidationMultiple(errors) => {
                    for error in errors {
                        eprintln!("{}", error);
                    }
                }
                error => {
                    eprintln!("{}", error);
                }
            }
            std::process::exit(INVALID_ARGS_CONFIG_EXIT_CODE);
        }
    }
}
