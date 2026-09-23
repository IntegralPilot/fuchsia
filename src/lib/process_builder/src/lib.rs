// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Native process creation and program loading library.
//!
//! # Restrictions
//!
//! Most Fuchsia processes are not able to use this library.
//!
//! This library uses the [zx_process_create] syscall to create a new process in a job. Use of that
//! syscall requires that the job of the process using the syscall (not the job that the process is
//! being created in) be allowed to create processes. In concrete terms, the process using this
//! library must be in a job whose [ZX_POL_NEW_PROCESS job policy is
//! ZX_POL_ACTION_ALLOW][zx_job_set_policy].
//!
//! Most processes on Fuchsia run in jobs where this job policy is set to DENY and thus will not
//! be able to use this library.  Those processes should instead use the [fuchsia.process.Launcher]
//! FIDL service, which is itself implemented using this library. [fdio::spawn()],
//! [fdio::spawn_vmo()], and [fdio::spawn_etc()] provide simple interfaces to this service.
//!
//! # Example
//!
//! ```
//! let process_name = CString::new("my_process")?;
//! let job = /* job to create new process in */;
//! let executable = /* VMO with execute rights containing ELF executable */;
//! let pkg_directory = /* fidl::endpoints::ClientEnd for fuchsia.io.Directory */;
//! let out_directory = /* server end of zx::Channel */;
//! let other_handle = /* some arbitrary zx::Handle */;
//!
//! let builder = ProcessBuilder::new(&process_name, &job, executable)?;
//! builder.add_arguments(vec![process_name, CString::new("arg0")?]);
//! builder.add_environment_variables(vec![CString::new("VAR=VALUE")?]);
//! builder.add_namespace_entries(vec![NamespaceEntry{
//!     path: CString::new("/pkg")?,
//!     directory: package_directory,
//! }])?;
//! builder.add_handles(vec![
//!     StartupHandle{
//!         handle: out_directory.into_handle(),
//!         info: HandleInfo::new(HandleType::DirectoryRequest, 0),
//!     },
//!     StartupHandle{
//!         handle: other_handle,
//!         info: HandleInfo::new(HandleType::User0, 1),
//!     },
//! ])?;
//!
//! let built_process: BuiltProcess = builder.build()?;
//! let process: zx::Process = builder.start()?;
//! ```
//!
//! [zx_process_create]: https://fuchsia.dev/fuchsia-src/reference/syscalls/process_create.md
//! [zx_job_set_policy]: https://fuchsia.dev/fuchsia-src/reference/syscalls/job_set_policy.md
//! [fuchsia.process.Launcher]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/zircon/system/fidl/fuchsia-process/launcher.fidl

pub use self::process_args::*;
pub use self::process_builder::*;

pub mod elf_load;

mod process_args;
mod process_builder;
mod util;
