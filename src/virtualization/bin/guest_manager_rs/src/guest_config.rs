// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42066994): Remove.
#![allow(unused_variables, unused_imports, dead_code)]

use anyhow::{Error, anyhow};
use fidl::endpoints::{ClientEnd, ProtocolMarker, ServerEnd};
use fidl_fuchsia_virtualization::{
    BlockFormat, BlockMode, BlockSpec, GuestConfig, KernelType, MAX_BLOCK_DEVICE_ID,
};
use fuchsia_fs::{PERM_READABLE, file};
use serde::{Deserialize, de};
use std::path::Path;
use {fidl_fuchsia_io as fio, static_assertions as sa};

// Memory is specified by a string containing either a plain u64 value in bytes, or a u64
// followed by an optional unit suffix.
// Supported suffixes:
// 'b': bytes (default if none specified)
// 'k': 2^10 bytes
// 'M': 2^20 bytes
// 'G': 2^30 bytes
// Examples: 2G, 65536, 512M
fn deserialize_mem<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: de::Deserializer<'de>,
{
    let s: Option<&str> = Option::deserialize(deserializer)?;
    match s {
        Some(s) => {
            if s.is_empty() {
                return Err(de::Error::invalid_length(s.len(), &"a non-empty string"));
            };
            let mut s_it = s.chars();
            let suffix = s_it.next_back().unwrap();

            let unit_scale = match suffix {
                'b' => Ok(1),
                'k' => Ok(1 << 10),
                'M' => Ok(1 << 20),
                'G' => Ok(1 << 30),
                _ if suffix.is_ascii_digit() => Ok(1),
                _ => Err(de::Error::custom("Invalid unit suffix")),
            }?;
            if suffix.is_ascii_digit() { s.parse::<u64>() } else { s_it.as_str().parse::<u64>() }
                .map_err(|_| {
                    de::Error::invalid_value(de::Unexpected::Str(s), &"a valid memory format")
                })
                .and_then(|num| {
                    num.checked_mul(unit_scale)
                        .ok_or_else(|| de::Error::custom("value would overflow u64"))
                })
                .map(Some)
        }
        None => Ok(None),
    }
}

// Intermediate structure representing the json configuration format.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct JsonConfig<'a> {
    block: Option<Vec<&'a str>>,
    cmdline: Option<&'a str>,
    cpus: Option<u8>,
    dtb_overlay: Option<&'a str>,
    linux: Option<&'a str>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_mem")]
    memory: Option<u64>,
    ramdisk: Option<&'a str>,
    zircon: Option<&'a str>,
    default_net: Option<bool>,
    virtio_balloon: Option<bool>,
    virtio_console: Option<bool>,
    virtio_gpu: Option<bool>,
    virtio_rng: Option<bool>,
    virtio_sound: Option<bool>,
    virtio_sound_input: Option<bool>,
    virtio_vsock: Option<bool>,
}

fn open_as_client_end<M: ProtocolMarker>(path: &str) -> Result<ClientEnd<M>, Error> {
    let (client_end, server_end) = fidl::Channel::create();
    file::open_channel_in_namespace(
        path,
        PERM_READABLE,
        ServerEnd::<fio::FileMarker>::new(server_end),
    )?;
    Ok(ClientEnd::<M>::new(client_end))
}

fn open_at<M: ProtocolMarker>(dir: &Path, fpath: &str) -> Result<ClientEnd<M>, Error> {
    open_as_client_end::<M>(
        dir.join(fpath).to_str().ok_or_else(|| anyhow!("file path is not a valid UTF-8 string"))?,
    )
}

// Blockid is the last MAX_BLOCK_DEVICE_ID bytes of the block filepath.
// In the case this falls within a UTF8 codepoint we reduce the length of the id to the
// nearest char boundary.
fn block_id(path: &str) -> &str {
    const UTF8_MAX_BYTES: u8 = 4;
    sa::const_assert!(MAX_BLOCK_DEVICE_ID >= UTF8_MAX_BYTES);
    let mut pos = path.len().checked_sub(MAX_BLOCK_DEVICE_ID as usize).unwrap_or(0);
    while !path.is_char_boundary(pos) {
        pos += 1;
    }
    &path[pos..]
}

// Parse a blockspec string consisting of a filepath followed by a comma separated list of
// options.
// Supported options: rw,ro,volatile,file,qcow,block
// Example: "data/filesystem.img,ro,volatile"
fn parse_block_spec(spec: &str, dir: &Path) -> Result<BlockSpec, Error> {
    enum BlockFmt {
        File,
        Qcow,
        Block,
    }

    // Enforce that filepath comes first and signal an error on other unknown flags.
    let mut spec_it = spec.split(',');
    if let (Some(fpath), (Some(block_mode), block_format)) = (
        spec_it.next(),
        spec_it.try_fold((Option::<BlockMode>::None, BlockFmt::File), |(bm, bf), s| match s {
            "rw" => Ok((Some(BlockMode::ReadWrite), bf)),
            "ro" => Ok((Some(BlockMode::ReadOnly), bf)),
            "volatile" => Ok((Some(BlockMode::VolatileWrite), bf)),
            "file" => Ok((bm, BlockFmt::File)),
            "qcow" => Ok((bm, BlockFmt::Qcow)),
            "block" => Ok((bm, BlockFmt::Block)),
            _ => Err(zx::Status::INVALID_ARGS),
        })?,
    ) {
        Ok(BlockSpec {
            id: block_id(fpath).to_string(),
            mode: block_mode,
            format: match block_format {
                BlockFmt::File => BlockFormat::File(open_at(dir, fpath)?),
                BlockFmt::Qcow => {
                    BlockFormat::Qcow(open_at::<fio::FileMarker>(dir, fpath)?.into_channel())
                }
                BlockFmt::Block => BlockFormat::Block(open_at(dir, fpath)?),
            },
        })
    } else {
        Err(zx::Status::INVALID_ARGS.into())
    }
}

pub fn parse_config(data: &str, dir: &Path) -> Result<GuestConfig, Error> {
    let conf: JsonConfig<'_> = serde_json::from_str(data)?;
    let kernel = match (conf.zircon, conf.linux) {
        (Some(_), Some(_)) => Err(zx::Status::INVALID_ARGS),
        (Some(z), _) => Ok(Some((KernelType::Zircon, z))),
        (_, Some(l)) => Ok(Some((KernelType::Linux, l))),
        _ => Ok(None),
    }?;

    Ok(GuestConfig {
        kernel_type: kernel.map(|(k, _)| k),
        kernel: kernel.map(|(_, k)| open_at(dir, k)).transpose()?,
        ramdisk: conf.ramdisk.map(|s| open_at(dir, s)).transpose()?,
        dtb_overlay: conf.dtb_overlay.map(|s| open_at(dir, s)).transpose()?,
        cmdline: conf.cmdline.map(|s| s.to_string()),
        cpus: conf.cpus,
        guest_memory: conf.memory,
        block_devices: conf
            .block
            .map(|bs| bs.iter().map(|s| parse_block_spec(s, dir)).collect::<Result<Vec<_>, _>>())
            .transpose()?,
        default_net: conf.default_net,
        virtio_balloon: conf.virtio_balloon,
        virtio_console: conf.virtio_console,
        virtio_gpu: conf.virtio_gpu,
        virtio_rng: conf.virtio_rng,
        virtio_vsock: conf.virtio_vsock,
        virtio_sound: conf.virtio_sound,
        virtio_sound_input: conf.virtio_sound_input,
        ..Default::default()
    })
}

// Merge two configs, with the overrides being applied on top of the base config. Non-repeated
// fields should be overwritten, and repeated fields should be appended.
pub fn merge_configs(mut base: GuestConfig, overrides: GuestConfig) -> GuestConfig {
    fn append_field<T>(base: Option<Vec<T>>, over: Option<Vec<T>>) -> Option<Vec<T>> {
        match (base, over) {
            (Some(mut b), Some(o)) => {
                b.extend(o);
                Some(b)
            }
            (Some(b), None) => Some(b),
            (None, Some(o)) => Some(o),
            (None, None) => None,
        }
    }
    fn override_field<T>(base: Option<T>, over: Option<T>) -> Option<T> {
        over.or(base)
    }

    // Apply fun to each listed field of base and override and assign the result to base's field
    // of the same name.
    macro_rules! merge_fields {
        ($fun:expr, $f:ident, $($fs:ident),+) => {
            merge_fields!($fun, $f);
            merge_fields!($fun, $($fs),+ )
        };
        ($fun:expr, $field:ident) => {
            base.$field = $fun(base.$field, overrides.$field)
        };
    }
    macro_rules! append_fields { ($($fs:ident),+) => { merge_fields!(append_field, $($fs),+) }}
    macro_rules! override_fields { ($($fs:ident),+) => { merge_fields!(override_field, $($fs),+) }}

    override_fields!(
        kernel_type,
        kernel,
        ramdisk,
        dtb_overlay,
        cmdline,
        cpus,
        guest_memory,
        default_net,
        virtio_balloon,
        virtio_console,
        virtio_gpu,
        virtio_rng,
        virtio_vsock,
        virtio_sound,
        virtio_sound_input,
        virtio_mem,
        virtio_mem_block_size,
        virtio_mem_region_size,
        virtio_mem_region_alignment
    );
    append_fields!(cmdline_add, block_devices, net_devices, vsock_listeners);

    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    // Empty strings are an error.
    #[fuchsia::test]
    async fn parse_empty_string() {
        assert!(parse_config("", Path::new("")).is_err());
    }

    // Parse empty but valid JSON.
    #[fuchsia::test]
    async fn parse_empty_config() {
        assert_eq!(GuestConfig::default(), parse_config("{}", Path::new("")).unwrap());
    }

    // Attempt to read files from incorrect guest directory.
    #[fuchsia::test]
    async fn parse_incorrect_guest_dir() {
        let tmpdir = tempdir().unwrap();
        let linux = tmpdir.path().join("kernel.img");
        let kernel_content = "this is not a kernel";
        let cfg = format!(r#"{{"linux": "kernel.img"}}"#,);
        // Create file to ensure error comes from missing directory.
        let flinux = file::open_in_namespace(
            linux.to_str().unwrap(),
            fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
        )
        .unwrap();
        flinux.write(kernel_content.as_bytes()).await.unwrap().unwrap();

        assert!(parse_config(&cfg, Path::new("")).is_err());
    }

    // Empty blockspecs are an error.
    #[fuchsia::test]
    async fn parse_block_spec_empty() {
        assert!(parse_block_spec("", Path::new("")).is_err());
    }

    // Blockspecs with invalid tokens result in an error.
    #[fuchsia::test]
    async fn parse_block_spec_error() {
        let invalid = "meow";
        assert!(parse_block_spec(&format!("filesystem.img,ro,{invalid}"), Path::new("")).is_err());
    }

    // Read contents of file attached to blockspec to string.
    async fn read_block_to_string(bs: BlockSpec) -> Result<String, Error> {
        if let BlockFormat::File(clientend) = bs.format {
            let proxy = clientend.into_proxy();
            Ok(file::read_to_string(&proxy).await?)
        } else {
            // Not handled: reading from Qcow or raw block devices
            unimplemented!()
        }
    }

    // Parse a valid blockspec
    #[fuchsia::test]
    async fn parse_block_spec_valid() -> Result<(), Error> {
        // Prepad filename with underscores to properly test blockid length limit.
        let fname = format!("{:_<1$}", "dummy_filesystem.img", (MAX_BLOCK_DEVICE_ID + 10) as usize);
        let args = "ro,file";
        let fcontent = "hello, this is a test";
        let tmpdir = tempdir().unwrap();
        let fpath = tmpdir.path().join(&fname);
        let tmpfile = file::open_in_namespace(
            fpath.to_str().unwrap(),
            fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
        )?;
        tmpfile.write(fcontent.as_bytes()).await?.unwrap();

        let bs = parse_block_spec(&format!("{},{}", fname, args), tmpdir.path()).unwrap();
        assert_eq!(bs.id, block_id(fpath.to_str().unwrap()));
        assert_eq!(bs.mode, BlockMode::ReadOnly);
        assert_eq!(read_block_to_string(bs).await?, fcontent);
        Ok(())
    }

    // Parse a config with several basic types.
    #[fuchsia::test]
    async fn parse_simple_config() -> Result<(), Error> {
        let tmpdir = tempdir().unwrap();
        let cmdline = "root=/dev/vda rw systemd.log_target=kmsg";
        let linux = "kernel.img";
        let kernel_content = "this is not a kernel";
        let default_net = true;
        let cpus = 4;
        let cfg = format!(
            r#"{{
    "cmdline": "{cmdline}",
    "linux": "{linux}",
    "default-net": {default_net},
    "memory": "2G",
    "cpus": {cpus}}}"#,
        );

        let flinux = file::open_in_namespace(
            tmpdir.path().join(linux).to_str().unwrap(),
            fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
        )?;
        flinux.write(kernel_content.as_bytes()).await?.unwrap();

        let guest_cfg = parse_config(&cfg, tmpdir.path())?;
        assert_eq!(guest_cfg.cpus, Some(4));
        assert_eq!(guest_cfg.default_net, Some(default_net));
        assert_eq!(&guest_cfg.cmdline.unwrap(), cmdline);
        assert_eq!(guest_cfg.kernel_type, Some(KernelType::Linux));
        assert_eq!(guest_cfg.guest_memory, Some(2 * (1u64 << 30)));
        assert_eq!(
            &file::read_to_string(&guest_cfg.kernel.unwrap().into_proxy()).await?,
            kernel_content
        );
        Ok(())
    }

    // Parse a config with a list of block devices.
    #[fuchsia::test]
    async fn parse_array_config() -> Result<(), Error> {
        let tmpdir = tempdir().unwrap();
        let fss = ["filesystem.img", "anotherfs.img", "cow.img"]
            .iter()
            .map(|fs| tmpdir.path().join(fs))
            .collect::<Vec<PathBuf>>();
        for fs in fss.iter() {
            file::open_in_namespace(
                fs.to_str().unwrap(),
                fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
            )?;
        }
        let block1 = format!("{},ro,file", fss[0].to_str().unwrap());
        let block2 = format!("{},rw,block", fss[1].to_str().unwrap());
        let block3 = format!("{},qcow,volatile", fss[2].to_str().unwrap());
        let cfg = format!(
            r#"{{
    "block": [
        "{block1}",
        "{block2}",
        "{block3}"
    ]}}"#,
        );

        let guest_cfg = parse_config(&cfg, Path::new(""))?;
        assert_eq!(guest_cfg.block_devices.unwrap().len(), 3);
        Ok(())
    }

    #[fuchsia::test]
    async fn merge_simple_configs() -> Result<(), Error> {
        // Merge two configs without repeated fields.
        let tmpdir = tempdir().unwrap();
        let kernel = "kernel.img";
        file::open_in_namespace(
            tmpdir.path().join(kernel).to_str().unwrap(),
            fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
        )?;

        let cfgbase = GuestConfig {
            kernel_type: Some(KernelType::Zircon),
            cmdline: Some("root=/dev/vda rw systemd.log_target=kmsg".to_string()),
            default_net: Some(false),
            guest_memory: Some(4294967296),
            ..Default::default()
        };
        let cfgoverride = GuestConfig {
            default_net: Some(true),
            guest_memory: Some(65536),
            kernel: Some(open_as_client_end::<fio::FileMarker>(
                tmpdir.path().join(kernel).to_str().unwrap(),
            )?),
            ..Default::default()
        };

        let cfgmerge = merge_configs(cfgbase, cfgoverride);

        assert_eq!(cfgmerge.kernel_type, Some(KernelType::Zircon));
        assert!(cfgmerge.kernel.is_some());
        assert_eq!(&cfgmerge.cmdline.unwrap(), "root=/dev/vda rw systemd.log_target=kmsg");
        assert_eq!(cfgmerge.default_net, Some(true));
        assert_eq!(cfgmerge.guest_memory, Some(65536));

        Ok(())
    }

    #[fuchsia::test]
    async fn merge_configs_with_arrays() {
        // Merge two configs with repeated fields appended.
        let mut args1 = Vec::from(["cmdline", "args"].map(String::from));
        let args2 = Vec::from(["and", "more", "args"].map(String::from));
        let cfgbase = GuestConfig { cmdline_add: Some(args1.clone()), ..Default::default() };
        let cfgoverride = GuestConfig { cmdline_add: Some(args2.clone()), ..Default::default() };

        let cfgmerge = merge_configs(cfgbase, cfgoverride);
        args1.extend(args2);
        assert_eq!(cfgmerge.cmdline_add, Some(args1));
    }
}
