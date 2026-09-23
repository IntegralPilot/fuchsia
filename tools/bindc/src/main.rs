// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A Fuchsia Driver Bind Rules compiler

mod cpp_generator;
mod generate;
mod rust_generator;
use std::fmt::Write;

use anyhow::{Context, Error, anyhow};
use bind::compiler::{self, CompiledBindRules};
use bind::debugger::offline_debugger;
use bind::test;
use clap::{Args, Parser, Subcommand};
use std::fs::File;
use std::io::prelude::*;
use std::io::{self, Write as IoWrite};
use std::path::PathBuf;

#[derive(Args, Debug)]
struct SharedOptions {
    /// The bind library input files. These may be included by the bind rules. They should be in
    /// the format described in //tools/bindc/README.md.
    #[arg(short = 'i', long = "include", value_delimiter = ' ', num_args = 1.., required = false)]
    include: Vec<PathBuf>,

    /// Specifiy the bind library input files as a file. The file must contain a list of filenames
    /// that are bind library input files that may be included by the bind rules. Those files
    /// should be in the format described in //tools/bindc/README.md.
    #[arg(short = 'f', long = "include-file")]
    include_file: Option<PathBuf>,

    /// The bind rules input file. This should be in the format described in
    /// //tools/bindc/README.md. This is required unless disable_autobind is true, in which case
    /// the driver while bind unconditionally (but only on the user's request.)
    input: Option<PathBuf>,

    /// Check inputs for style guide violations.
    #[arg(short = 'l', long = "lint")]
    lint: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(name = "compile")]
    Compile {
        #[command(flatten)]
        options: SharedOptions,

        /// Output file. The compiler emits a C header file.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,

        /// Specify a path for the compiler to generate a depfile. A depfile contain, in Makefile
        /// format, the files that this invocation of the compiler depends on including all bind
        /// libraries and the bind rules input itself. An output file must be provided to generate
        /// a depfile.
        #[arg(short = 'd', long = "depfile")]
        depfile: Option<PathBuf>,

        // TODO(https://fxbug.dev/42119701): Eventually this option should be removed when we can define this
        // configuration in the driver's component manifest.
        /// Disable automatically binding the driver so that the driver must be bound on a user's
        /// request.
        #[arg(short = 'a', long = "disable-autobind")]
        disable_autobind: bool,
    },
    #[command(name = "debug")]
    Debug {
        #[command(flatten)]
        options: SharedOptions,

        /// A file containing the properties of a specific device, as a list of key-value pairs.
        /// This will be used as the input to the bind rules debugger.
        #[arg(short = 'd', long = "debug")]
        device_file: PathBuf,
    },
    #[command(name = "test")]
    Test {
        #[command(flatten)]
        options: SharedOptions,

        // TODO(https://fxbug.dev/42134547): Refer to documentation for bind testing.
        /// A file containing the test specification.
        #[arg(short = 't', long = "test-spec")]
        test_spec: PathBuf,
    },
    /// Generate a C++ header file based on the input Bind Library file.
    #[command(name = "generate-cpp")]
    GenerateCpp {
        /// The Bind Library input file.
        input: PathBuf,

        /// Check the input for style guide violations.
        #[arg(short = 'l', long = "lint")]
        lint: bool,

        /// Output C++ header file.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    /// Generate a Rust file based on the input Bind Library file.
    #[command(name = "generate-rust")]
    GenerateRust {
        /// The Bind Library input file.
        input: PathBuf,

        /// Check the input for style guide violations.
        #[arg(short = 'l', long = "lint")]
        lint: bool,

        /// Output Rust file.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
}

#[derive(Parser, Debug)]
struct Opts {
    #[command(subcommand)]
    command: Command,
}

fn main() {
    let opts = Opts::parse();
    if let Err(err) = handle_command(opts.command) {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}

fn write_depfile(
    output: &PathBuf,
    input: &Option<PathBuf>,
    includes: &[PathBuf],
) -> Result<String, Error> {
    fn path_to_str(path: &PathBuf) -> Result<&str, Error> {
        path.as_os_str().to_str().context("failed to convert path to string")
    }

    let mut deps = includes.iter().map(|s| path_to_str(s)).collect::<Result<Vec<&str>, Error>>()?;

    if let Some(input) = input {
        let input_str = path_to_str(input)?;
        deps.push(input_str);
    }

    let output_str = path_to_str(output)?;
    let mut out = String::new();
    writeln!(&mut out, "{}: {}", output_str, deps.join(" "))?;
    Ok(out)
}

fn read_file(path: &PathBuf) -> Result<String, Error> {
    let mut file = File::open(path)?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf)
}

pub enum GeneratedBindingType {
    Cpp,
    Rust,
}

pub fn handle_code_generate(
    binding_type: GeneratedBindingType,
    input: PathBuf,
    lint: bool,
    output: Option<PathBuf>,
) -> Result<(), Error> {
    let input_content = read_file(&input)?;

    // Generate the target.
    let generated_content = generate::generate(binding_type, &input_content, lint)?;

    // Create and open output file.
    let mut output_writer: Box<dyn io::Write> = if let Some(output) = output {
        Box::new(File::create(output).context("Failed to create output file.")?)
    } else {
        // Output file name was not given. Print result to stdout.
        Box::new(io::stdout())
    };

    // Write generated target to output.
    output_writer
        .write_all(generated_content.as_bytes())
        .context("Failed to write to output file")?;

    Ok(())
}

fn handle_command(command: Command) -> Result<(), Error> {
    match command {
        Command::Debug { options, device_file } => {
            let includes = handle_includes(options.include, options.include_file)?;
            let includes = includes.iter().map(read_file).collect::<Result<Vec<String>, _>>()?;
            let input =
                options.input.ok_or_else(|| anyhow!("The debug command requires an input."))?;
            let rules = read_file(&input)?;
            let bind_rules =
                compiler::compile_bind(&rules, &includes, options.lint, false, false, false)?;

            let device = read_file(&device_file)?;
            let binds = offline_debugger::debug_from_str(&bind_rules, &device)?;
            if binds {
                println!("Driver binds to device.");
            } else {
                println!("Driver doesn't bind to device.");
            }
            Ok(())
        }
        Command::Test { options, test_spec } => {
            let input =
                options.input.ok_or_else(|| anyhow!("The test command requires an input."))?;
            let rules = read_file(&input)?;
            let includes = handle_includes(options.include, options.include_file)?;
            let includes = includes.iter().map(read_file).collect::<Result<Vec<String>, _>>()?;
            let test_spec = read_file(&test_spec)?;
            if !test::run(&rules, &includes, &test_spec)? {
                return Err(anyhow!("Test failed"));
            }
            Ok(())
        }
        Command::Compile { options, output, depfile, disable_autobind } => {
            let includes = handle_includes(options.include, options.include_file)?;
            handle_compile(options.input, includes, disable_autobind, options.lint, output, depfile)
        }
        Command::GenerateCpp { input, lint, output } => {
            handle_code_generate(GeneratedBindingType::Cpp, input, lint, output)
        }
        Command::GenerateRust { input, lint, output } => {
            handle_code_generate(GeneratedBindingType::Rust, input, lint, output)
        }
    }
}

fn handle_includes(
    mut includes: Vec<PathBuf>,
    include_file: Option<PathBuf>,
) -> Result<Vec<PathBuf>, Error> {
    if let Some(include_file) = include_file {
        let file = File::open(include_file).context("Failed to open include file")?;
        let reader = io::BufReader::new(file);
        let mut filenames = reader
            .lines()
            .map(|line| line.map(PathBuf::from))
            .map(|line| line.context("Failed to read include file"))
            .collect::<Result<Vec<_>, Error>>()?;
        includes.append(&mut filenames);
    }
    Ok(includes)
}

fn handle_compile(
    input: Option<PathBuf>,
    includes: Vec<PathBuf>,
    disable_autobind: bool,
    lint: bool,
    output: Option<PathBuf>,
    depfile: Option<PathBuf>,
) -> Result<(), Error> {
    let mut output_writer: Box<dyn io::Write> = if let Some(output) = output {
        // If there's an output filename then we can generate a depfile too.
        if let Some(filename) = depfile {
            let mut file = File::create(filename).context("Failed to open depfile")?;
            let depfile_string =
                write_depfile(&output, &input, &includes).context("Failed to create depfile")?;
            file.write(depfile_string.as_bytes()).context("Failed to write to depfile")?;
        }
        Box::new(File::create(output).context("Failed to create output file")?)
    } else {
        Box::new(io::stdout())
    };

    let rules_str;
    let compiled_bind_rules = if !disable_autobind {
        let input =
            input.ok_or_else(|| anyhow!("An input is required when disable_autobind is false."))?;
        rules_str = read_file(&input)?;
        let includes = includes.iter().map(read_file).collect::<Result<Vec<String>, _>>()?;
        compiler::compile(&rules_str, &includes, lint, disable_autobind, true, false)?
    } else if let Some(input) = input {
        // Autobind is disabled but there are some bind rules for manual binding.
        rules_str = read_file(&input)?;
        let includes = includes.iter().map(read_file).collect::<Result<Vec<String>, _>>()?;
        let compiled_bind_rules =
            compiler::compile(&rules_str, &includes, lint, disable_autobind, true, false)?;
        compiled_bind_rules
    } else {
        CompiledBindRules::empty_bind_rules(disable_autobind, true, false)
    };

    let bytecode = compiled_bind_rules.encode_to_bytecode()?;
    output_writer.write_all(bytecode.as_slice()).context("Failed to write to output file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bind::compiler::{BindRules, SymbolicInstruction, SymbolicInstructionInfo};
    use std::collections::HashMap;

    #[test]
    fn zero_instructions() {
        let bind_rules = CompiledBindRules::Bind(BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        });
        assert_eq!(
            bind_rules.encode_to_bytecode().unwrap(),
            vec![
                66, 73, 78, 68, 2, 0, 0, 0, 0, 83, 89, 78, 66, 0, 0, 0, 0, 73, 78, 83, 84, 0, 0, 0,
                0
            ]
        );
    }

    #[test]
    fn one_instruction() {
        let bind_rules = CompiledBindRules::Bind(BindRules {
            instructions: vec![SymbolicInstructionInfo {
                location: None,
                instruction: SymbolicInstruction::UnconditionalAbort,
            }],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        });
        assert_eq!(
            bind_rules.encode_to_bytecode().unwrap(),
            vec![
                66, 73, 78, 68, 2, 0, 0, 0, 0, 83, 89, 78, 66, 0, 0, 0, 0, 73, 78, 83, 84, 1, 0, 0,
                0, 48
            ]
        );
    }

    #[test]
    fn depfile_no_includes() {
        let output = PathBuf::from("/a/output");
        let input = PathBuf::from("/a/input");
        assert_eq!(
            write_depfile(&output, &Some(input), &[]).unwrap(),
            "/a/output: /a/input\n".to_string()
        );
    }

    #[test]
    fn depfile_no_input() {
        let output = PathBuf::from("/a/output");
        let includes = vec![PathBuf::from("/a/include"), PathBuf::from("/b/include")];
        let result = write_depfile(&output, &None, &includes).unwrap();
        assert!(result.starts_with("/a/output:"));
        assert!(result.contains("/a/include"));
        assert!(result.contains("/b/include"));
    }

    #[test]
    fn depfile_input_and_includes() {
        let output = PathBuf::from("/a/output");
        let input = PathBuf::from("/a/input");
        let includes = vec![PathBuf::from("/a/include"), PathBuf::from("/b/include")];
        let result = write_depfile(&output, &Some(input), &includes).unwrap();
        assert!(result.starts_with("/a/output:"));
        assert!(result.contains("/a/input"));
        assert!(result.contains("/a/include"));
        assert!(result.contains("/b/include"));
    }
}
