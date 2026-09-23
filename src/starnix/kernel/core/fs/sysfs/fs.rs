// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::sysfs::{build_cpu_class_directory, build_kernel_directory, build_power_directory};
use crate::task::{CurrentTask, Kernel};
use crate::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use crate::vfs::pseudo::simple_file::BytesFile;
use crate::vfs::pseudo::stub_empty_file::StubEmptyFile;
use crate::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsStr,
};
use ebpf_api::BPF_PROG_TYPE_FUSE;
use starnix_logging::bug_ref;
use starnix_types::vfs::default_statfs;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::{SYSFS_MAGIC, statfs};

struct SysFs;
impl FileSystemOps for SysFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(SYSFS_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "sysfs".into()
    }
}

impl SysFs {
    fn new_fs(kernel: &Kernel, options: FileSystemOptions) -> FileSystemHandle {
        let fs =
            FileSystem::new(kernel, CacheMode::Cached(kernel.fs_cache_config()), SysFs, options)
                .expect("sysfs constructed with valid options");

        fn empty_dir(_: &SimpleDirectoryMutator) {}

        let registry = &kernel.device_registry;
        let root = &registry.objects.root;
        fs.create_root(fs.allocate_ino(), root.clone());
        let dir = SimpleDirectoryMutator::new(fs.clone(), root.clone());

        let dir_mode = 0o755;
        dir.subdir("fs", dir_mode, |dir| {
            dir.subdir("selinux", dir_mode, empty_dir);
            dir.subdir("bpf", dir_mode, empty_dir);
            dir.subdir("cgroup", dir_mode, empty_dir);
            dir.subdir("fuse", dir_mode, |dir| {
                dir.subdir("connections", dir_mode, empty_dir);
                dir.subdir("features", dir_mode, |dir| {
                    dir.entry(
                        "fuse_bpf",
                        BytesFile::new_node(b"supported\n".to_vec()),
                        mode!(IFREG, 0o444),
                    );
                });
                dir.entry(
                    "bpf_prog_type_fuse",
                    BytesFile::new_node(format!("{}\n", BPF_PROG_TYPE_FUSE).into_bytes()),
                    mode!(IFREG, 0o444),
                );
            });
            dir.subdir("pstore", dir_mode, empty_dir);
        });

        dir.subdir("block", dir_mode, |dir| {
            dir.subdir("zram0", dir_mode, |dir| {
                dir.entry(
                    "backing_dev",
                    StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                    mode!(IFREG, 0o444),
                );
                dir.entry(
                    "recomp_algorithm",
                    StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                    mode!(IFREG, 0o444),
                );
                dir.entry(
                    "recompress",
                    StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                    mode!(IFREG, 0o444),
                );
            });
        });

        dir.subdir("bus", dir_mode, |dir| {
            dir.subdir("mmc", dir_mode, |dir| {
                dir.subdir("devices", dir_mode, |dir| {
                    dir.subdir("mmc0:0001", dir_mode, |dir| {
                        dir.subdir("block", dir_mode, |dir| {
                            dir.subdir("mmcblk0", dir_mode, |dir| {
                                dir.entry(
                                    "size",
                                    StubEmptyFile::new_node(bug_ref!(
                                        "https://fxbug.dev/452096300"
                                    )),
                                    mode!(IFREG, 0o444),
                                );
                            });
                        });
                    });
                });
            });
            dir.subdir("platform", dir_mode, |dir| {
                dir.subdir("drivers", dir_mode, |dir| {
                    dir.entry(
                        "trusty",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                });
            });
        });

        dir.subdir("class", dir_mode, |dir| {
            dir.subdir("backlight", dir_mode, |dir| {
                dir.subdir("panel0-backlight", dir_mode, |dir| {
                    dir.entry(
                        "brightness",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                });
            });
            dir.subdir("bdi", dir_mode, |dir| {
                dir.subdir("0:80", dir_mode, |dir| {
                    dir.entry(
                        "max_ratio",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                    dir.entry(
                        "read_ahead_kb",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                });
            });
            dir.subdir("mmc_host", dir_mode, |dir| {
                dir.subdir("mmc0", dir_mode, |dir| {
                    dir.subdir("mmc0:0001", dir_mode, |dir| {
                        dir.entry(
                            "fwrev",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "hwrev",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "life_time",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "manfid",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "pre_eol_info",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "serial",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                    });
                });
            });
            dir.subdir("net", dir_mode, |dir| {
                dir.subdir("eth0", dir_mode, |dir| {
                    dir.entry(
                        "address",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                });
                dir.subdir("sit0", dir_mode, |dir| {
                    dir.entry(
                        "address",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                });
                dir.subdir("wlan0", dir_mode, |dir| {
                    dir.entry(
                        "address",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o444),
                    );
                });
            });
            dir.subdir("powercap", dir_mode, |_dir| {});
            dir.subdir("udc", dir_mode, |_dir| {});
        });
        dir.subdir("dev", dir_mode, |dir| {
            dir.subdir("char", dir_mode, empty_dir);
            dir.subdir("block", dir_mode, empty_dir);
        });
        dir.subdir("firmware", dir_mode, |dir| {
            dir.subdir("devicetree", dir_mode, |dir| {
                dir.subdir("base", dir_mode, |dir| {
                    dir.subdir("chosen", dir_mode, |dir| {
                        dir.subdir("plat", dir_mode, |dir| {
                            if let Some(device_tree) = &kernel.device_tree {
                                if let Some(product_bytes) =
                                    device_tree.root_node.find("plat").and_then(|n| {
                                        n.get_property("product").map(|p| p.value.clone())
                                    })
                                {
                                    let product_bytes = if product_bytes.len() >= 4 {
                                        product_bytes.to_vec()
                                    } else {
                                        let mut padded_bytes = vec![0; 4];
                                        let start = 4 - product_bytes.len();
                                        padded_bytes[start..].copy_from_slice(&product_bytes);
                                        padded_bytes
                                    };
                                    dir.entry(
                                        "product",
                                        BytesFile::new_node(product_bytes),
                                        mode!(IFREG, 0o444),
                                    );
                                }
                            }
                        });
                        dir.subdir("config", dir_mode, |dir| {
                            dir.entry(
                                "pcbcfg",
                                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                                mode!(IFREG, 0o444),
                            );
                        });
                    });
                    dir.subdir("firmware", dir_mode, |dir| {
                        dir.subdir("android", 0o755, |dir| {
                            dir.entry(
                                "compatible",
                                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                                mode!(IFREG, 0o444),
                            );
                            dir.subdir("vbmeta", 0o755, |dir| {
                                dir.entry(
                                    "parts",
                                    StubEmptyFile::new_node(bug_ref!(
                                        "https://fxbug.dev/452096300"
                                    )),
                                    mode!(IFREG, 0o444),
                                );
                            });
                        });
                    });
                    dir.subdir("mcu", dir_mode, |dir| {
                        dir.entry(
                            "board_type",
                            BytesFile::new_node(b"starnix".to_vec()),
                            mode!(IFREG, 0o444),
                        );
                    });
                });
            });
        });

        dir.subdir("kernel", dir_mode, |dir| {
            build_kernel_directory(kernel, dir);
        });

        dir.subdir("power", 0o755, |dir| {
            build_power_directory(kernel, dir);
        });

        dir.subdir("leds", dir_mode, |dir| {
            dir.entry(
                "leds",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
        });

        dir.subdir("module", dir_mode, |dir| {
            dir.subdir("dm_bufio", dir_mode, |dir| {
                dir.subdir("parameters", dir_mode, |dir| {
                    dir.entry(
                        "max_age_seconds",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o644),
                    );
                });
            });
            dir.subdir("dm_verity", dir_mode, |dir| {
                dir.subdir("parameters", dir_mode, |dir| {
                    dir.entry(
                        "prefetch_cluster",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/322893670")),
                        mode!(IFREG, 0o644),
                    );
                });
            });
        });

        // TODO(https://fxbug.dev/425942145): Correctly implement system filesystem in sysfs
        dir.subdir("devices", dir_mode, |dir| {
            dir.subdir("system", dir_mode, |dir| {
                dir.subdir("cpu", dir_mode, build_cpu_class_directory);
            });
            dir.subdir("leds", dir_mode, |_dir| {});
            dir.subdir("platform", dir_mode, |dir| {
                dir.subdir("soc", dir_mode, |dir| {
                    dir.subdir("1c40000.qcom,spmi", dir_mode, |dir| {
                        dir.subdir("spmi-0", dir_mode, |dir| {
                            dir.subdir("0-00", dir_mode, |dir| {
                                dir.subdir(
                                    "1c40000.qcom,spmi:qcom,pm5100@0:qpnp,qbg@4f00",
                                    dir_mode,
                                    |dir| {
                                        dir.subdir("iio:device3", dir_mode, |dir| {
                                            dir.entry(
                                                "in_resistance_resistance_id_input",
                                                StubEmptyFile::new_node(bug_ref!(
                                                    "https://fxbug.dev/452096300"
                                                )),
                                                mode!(IFREG, 0o444),
                                            );
                                        });
                                    },
                                );
                            });
                        });
                    });
                    dir.subdir("5e00000.qcom,mdss_mdp", dir_mode, |dir| {
                        dir.subdir("drm", dir_mode, |dir| {
                            dir.subdir("card0", dir_mode, |dir| {
                                dir.subdir("sde-conn-0-DSI-1", dir_mode, |dir| {
                                    dir.entry(
                                        "display_power_state",
                                        StubEmptyFile::new_node(bug_ref!(
                                            "https://fxbug.dev/452096300"
                                        )),
                                        mode!(IFREG, 0o644),
                                    );
                                    dir.entry(
                                        "panel_power_state",
                                        StubEmptyFile::new_node(bug_ref!(
                                            "https://fxbug.dev/452096300"
                                        )),
                                        mode!(IFREG, 0o644),
                                    );
                                });
                            });
                        });
                    });
                });
            });
            dir.subdir("soc0", dir_mode, |dir| {
                dir.entry(
                    "revision",
                    StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                    mode!(IFREG, 0o444),
                );
                dir.entry(
                    "serial_number",
                    StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                    mode!(IFREG, 0o444),
                );
            });
            dir.subdir("virtual", dir_mode, |dir| {
                dir.subdir("leds", dir_mode, |_dir| {});
                dir.subdir("power_supply", dir_mode, |dir| {
                    dir.subdir("bms", dir_mode, |dir| {
                        dir.entry(
                            "capacity",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "capacity_level",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                        dir.entry(
                            "status",
                            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                            mode!(IFREG, 0o444),
                        );
                    });
                });
            });
        });

        fs
    }
}

struct SysFsHandle(FileSystemHandle);

pub fn sys_fs(
    current_task: &CurrentTask,
    _options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    Ok(get_sysfs(current_task.kernel()))
}

pub fn get_sysfs(kernel: &Kernel) -> FileSystemHandle {
    kernel
        .expando
        .get_or_init(|| SysFsHandle(SysFs::new_fs(kernel, FileSystemOptions::default())))
        .0
        .clone()
}
