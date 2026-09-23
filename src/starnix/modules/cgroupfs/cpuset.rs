// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::{CgroupOps, CurrentTask};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::borrow::Cow;
use std::sync::{Arc, Weak};
use zx;

pub struct CpusetCpusFile {
    cgroup: Weak<dyn CgroupOps>,
}

impl CpusetCpusFile {
    pub fn new_node(cgroup: Weak<dyn CgroupOps>) -> impl FsNodeOps {
        BytesFile::new_node(Self { cgroup })
    }

    fn cgroup(&self) -> Result<Arc<dyn CgroupOps>, Errno> {
        self.cgroup.upgrade().ok_or_else(|| errno!(ENODEV))
    }
}

fn parse_cpu_list(s: &str) -> Result<Vec<u32>, Errno> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let max_cpu = zx::system_get_num_cpus();
    let mut cpus = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start = start_str.trim().parse::<u32>().map_err(|_| errno!(EINVAL))?;
            let end = end_str.trim().parse::<u32>().map_err(|_| errno!(EINVAL))?;
            if start > end {
                return error!(EINVAL);
            }
            if end >= max_cpu {
                return error!(ERANGE);
            }
            for cpu in start..=end {
                cpus.push(cpu);
            }
        } else {
            let cpu = part.parse::<u32>().map_err(|_| errno!(EINVAL))?;
            if cpu >= max_cpu {
                return error!(ERANGE);
            }
            cpus.push(cpu);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

fn format_cpu_list(cpus: &[u32]) -> String {
    if cpus.is_empty() {
        return "".to_string();
    }
    let mut result = Vec::new();
    let mut start = cpus[0];
    let mut end = cpus[0];

    for &cpu in &cpus[1..] {
        if cpu == end + 1 {
            end = cpu;
        } else {
            if start == end {
                result.push(format!("{}", start));
            } else {
                result.push(format!("{}-{}", start, end));
            }
            start = cpu;
            end = cpu;
        }
    }

    if start == end {
        result.push(format!("{}", start));
    } else {
        result.push(format!("{}-{}", start, end));
    }

    result.join(",")
}

impl BytesFileOps for CpusetCpusFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let cpus_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let cpus = parse_cpu_list(cpus_str)?;
        let cgroup = self.cgroup()?;
        let cpuset = cgroup.cpuset().ok_or_else(|| errno!(ENODEV))?;
        cpuset.set_cpuset_cpus(cpus);
        Ok(())
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let cgroup = self.cgroup()?;
        let cpuset = cgroup.cpuset().ok_or_else(|| errno!(ENODEV))?;
        let cpus_str = format!("{}\n", format_cpu_list(cpuset.cpuset_cpus().as_slice()));
        Ok(cpus_str.as_bytes().to_owned().into())
    }
}
