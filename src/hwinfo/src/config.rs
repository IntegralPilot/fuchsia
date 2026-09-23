// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use chrono::NaiveDateTime;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_boot::{ItemsMarker, ItemsProxy};
use fidl_fuchsia_factory::MiscFactoryStoreProviderProxy;
use fidl_fuchsia_intl::{LocaleId, RegulatoryDomain};
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol;
use serde::{Deserialize, Serialize};
use std::borrow::BorrowMut;
use std::fs::File;
use std::io;

// CONFIG AND FACTORY FILE NAMES
const BOARD_CONFIG_JSON_FILE: &str = "/config/data/board_config.json";
const DEFAULT_BOARD_CONFIG_JSON_FILE: &str = "/config/data/default_board_config.json";
const SERIAL_TXT: &str = "serial.txt";
const LOCALE_LIST_FILE: &str = "locale_list.txt";
const HW_TXT: &str = "hw.txt";
const RETAIL_DEMO_FILE: &str = "demo_device";
// CONFIG KEYS
const SKU_KEY: &str = "config";
const LANGUAGE_KEY: &str = "lang";
const REGULATORY_DOMAIN_KEY: &str = "country";
const BUILD_DATE_KEY: &str = "mfg_date";
const BUILD_NAME_KEY: &str = "build_name";
const COLORWAY_KEY: &str = "color";
const DISPLAY_KEY: &str = "display";
const MEMORY_KEY: &str = "memory";
const NAND_STORAGE_KEY: &str = "nand";
const EMMC_STORAGE_KEY: &str = "emmc";
const MICROPHONE_KEY: &str = "mic";
const AUDIO_AMPLIFIER_KEY: &str = "amp";
// CONFIG VALUE FORMAT STRS
const BUILD_DATE_FORMAT_STR: &str = "%Y-%m-%dT%H:%M:%S";

async fn read_factory_file(
    path: &str,
    proxy_handle: &MiscFactoryStoreProviderProxy,
) -> Result<String, Error> {
    let (dir_proxy, dir_server_end) = create_proxy::<fio::DirectoryMarker>();
    proxy_handle.get_factory_store(dir_server_end)?;
    let file_proxy = fuchsia_fs::directory::open_file_async(&dir_proxy, path, fio::PERM_READABLE)?;
    let result = fuchsia_fs::file::read_to_string(&file_proxy).await?.trim().to_owned();
    return Ok(result);
}

async fn get_zbi_serial_number() -> Result<String, Error> {
    let boot: ItemsProxy = connect_to_protocol::<ItemsMarker>()?;

    let (vmo, sn_len) = boot.get(zbi::Type::SerialNumber.into(), 0).await?;

    if sn_len == 0 {
        return Err(anyhow::anyhow!("Serial number has length 0"));
    }

    if let Some(vmo) = vmo {
        let mut vec = vec![0u8; sn_len as usize];
        vmo.read(vec.borrow_mut(), 0).expect("reading VMO");
        return match String::from_utf8(vec) {
            Ok(v) => Ok(v),
            Err(err) => {
                log::warn!("Could not read Serial from VMO from Boot");
                Err(err.into())
            }
        };
    } else {
        log::warn!("Invalid VMO from Boot");
        return Err(anyhow::anyhow!("Invalid VMO from Boot"));
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceInfo {
    pub serial_number: Option<String>,
    pub is_retail_demo: bool,
    pub retail_sku: Option<String>,
}

impl DeviceInfo {
    pub async fn load(proxy_handle: &MiscFactoryStoreProviderProxy) -> Self {
        let mut device_info =
            DeviceInfo { serial_number: None, is_retail_demo: false, retail_sku: None };
        // First try to read the serial number from  SERIAL_TXT. This is
        // used in smart display products.
        device_info.serial_number = match read_factory_file(SERIAL_TXT, proxy_handle).await {
            Ok(content) => Some(content),
            Err(err) => {
                log::warn!("Failed to read factory file {}: {}", SERIAL_TXT, err);
                None
            }
        };
        // If the SERIAL_TXT is not present, try reading from boot. This is used
        // in vim3
        if device_info.serial_number.is_none() {
            device_info.serial_number = match get_zbi_serial_number().await {
                Ok(content) => Some(content),
                Err(err) => {
                    log::warn!("Failed to read serial number from boot {}", err);
                    None
                }
            };
        }
        if let Ok(content) = read_factory_file(RETAIL_DEMO_FILE, proxy_handle).await {
            device_info.is_retail_demo = true;
            device_info.retail_sku = Some(content)
        };
        device_info
    }
}

impl Into<fidl_fuchsia_hwinfo::DeviceInfo> for DeviceInfo {
    fn into(self) -> fidl_fuchsia_hwinfo::DeviceInfo {
        fidl_fuchsia_hwinfo::DeviceInfo {
            serial_number: self.serial_number,
            is_retail_demo: Some(self.is_retail_demo),
            retail_sku: self.retail_sku,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Architecture {
    X64,
    ARM64,
}

impl Into<fidl_fuchsia_hwinfo::Architecture> for Architecture {
    fn into(self) -> fidl_fuchsia_hwinfo::Architecture {
        match self {
            Architecture::X64 => fidl_fuchsia_hwinfo::Architecture::X64,
            Architecture::ARM64 => fidl_fuchsia_hwinfo::Architecture::Arm64,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BoardInfo {
    pub name: Option<String>,
    pub revision: Option<String>,
    pub cpu_architecture: Option<Architecture>,
}

impl BoardInfo {
    fn get_cpu_architecture() -> Option<Architecture> {
        match std::env::consts::ARCH {
            "x86_64" => Some(Architecture::X64),
            "aarch64" => Some(Architecture::ARM64),
            _ => None,
        }
    }

    fn read_config(path: &str) -> Result<Self, Error> {
        let board_info: BoardInfo = serde_json::from_reader(io::BufReader::new(File::open(path)?))?;
        Ok(board_info)
    }

    pub fn load() -> Self {
        let mut board_info = BoardInfo::read_config(BOARD_CONFIG_JSON_FILE).unwrap_or_else(|err| {
            log::error!("Failed to read board_config.json due to {}", err);
            BoardInfo::read_config(DEFAULT_BOARD_CONFIG_JSON_FILE).unwrap_or_else(|err| {
                log::error!("Failed to read default_board_config.json due to {}", err);
                BoardInfo { name: None, revision: None, cpu_architecture: None }
            })
        });
        board_info.cpu_architecture = BoardInfo::get_cpu_architecture();
        board_info
    }
}

impl Into<fidl_fuchsia_hwinfo::BoardInfo> for BoardInfo {
    fn into(self) -> fidl_fuchsia_hwinfo::BoardInfo {
        fidl_fuchsia_hwinfo::BoardInfo {
            name: self.name,
            revision: self.revision,
            cpu_architecture: match self.cpu_architecture {
                Some(val) => Some(Architecture::into(val)),
                _ => None,
            },
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProductInfo {
    pub sku: Option<String>,
    pub language: Option<String>,
    pub country_code: Option<String>,
    pub locales: Vec<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub build_date: Option<String>,
    pub build_name: Option<String>,
    pub colorway: Option<String>,
    pub display: Option<String>,
    pub memory: Option<String>,
    pub nand_storage: Option<String>,
    pub emmc_storage: Option<String>,
    pub microphone: Option<String>,
    pub audio_amplifier: Option<String>,
}

impl ProductInfo {
    fn new() -> Self {
        ProductInfo {
            sku: None,
            language: None,
            country_code: None,
            locales: Vec::new(),
            name: None,
            model: None,
            manufacturer: None,
            build_date: None,
            build_name: None,
            colorway: None,
            display: None,
            memory: None,
            nand_storage: None,
            emmc_storage: None,
            microphone: None,
            audio_amplifier: None,
        }
    }

    fn load_from_structured_config(&mut self) {
        let hwinfo = hwinfo_structured_config::Config::take_from_startup_handle();
        self.name = Some(hwinfo.product_name);
        self.model = Some(hwinfo.product_model);
        self.manufacturer = Some(hwinfo.product_manufacturer);
    }

    async fn load_from_hw_file(
        &mut self,
        path: &str,
        proxy_handle: &MiscFactoryStoreProviderProxy,
    ) -> Result<(), Error> {
        let file_content = read_factory_file(path, proxy_handle).await?;
        for config in file_content.lines() {
            let pair: Vec<_> = config.splitn(2, "=").collect();
            let key = pair[0];
            let value = pair[1];
            let mut filtered_value = value.to_string();
            match key {
                SKU_KEY => {
                    self.sku = Some(value.to_owned());
                }
                LANGUAGE_KEY => {
                    self.language = Some(value.to_owned());
                }
                REGULATORY_DOMAIN_KEY => {
                    self.country_code = Some(value.to_owned());
                }
                BUILD_DATE_KEY => {
                    self.build_date = Some(value.to_owned());
                    filtered_value = NaiveDateTime::parse_from_str(
                        self.build_date.as_ref().unwrap(),
                        BUILD_DATE_FORMAT_STR,
                    )
                    .map_or_else(
                        |_| "invalid_format".to_string(),
                        |datetime| format!("{}", datetime.date().format("%Y-%m-%d")),
                    );
                }
                BUILD_NAME_KEY => {
                    self.build_name = Some(value.to_owned());
                }
                COLORWAY_KEY => {
                    self.colorway = Some(value.to_owned());
                }
                DISPLAY_KEY => {
                    self.display = Some(value.to_owned());
                }
                MEMORY_KEY => {
                    self.memory = Some(value.to_owned());
                }
                NAND_STORAGE_KEY => {
                    self.nand_storage = Some(value.to_owned());
                }
                EMMC_STORAGE_KEY => {
                    self.emmc_storage = Some(value.to_owned());
                }
                MICROPHONE_KEY => {
                    self.microphone = Some(value.to_owned());
                }
                AUDIO_AMPLIFIER_KEY => {
                    self.audio_amplifier = Some(value.to_owned());
                }
                _ => {
                    log::warn!("hw.txt dictionary values {} - {}", key, value.to_owned());
                }
            }
            log::warn!("hw.txt line: {}={}", key, filtered_value);
        }
        Ok(())
    }

    async fn load_from_locale_list(
        &mut self,
        path: &str,
        proxy_handle: &MiscFactoryStoreProviderProxy,
    ) -> Result<(), Error> {
        let file_content = read_factory_file(path, proxy_handle).await?;
        self.locales = Vec::new();
        for line in file_content.lines() {
            self.locales.push(line.trim().to_owned());
        }
        Ok(())
    }

    pub async fn load(proxy_handle: &MiscFactoryStoreProviderProxy) -> Self {
        let mut product_info = ProductInfo::new();
        product_info.load_from_structured_config();
        if let Err(err) = product_info.load_from_hw_file(HW_TXT, proxy_handle).await {
            log::warn!("Failed to load hw.txt due to {}", err);
        }
        if let Err(err) = product_info.load_from_locale_list(LOCALE_LIST_FILE, proxy_handle).await {
            log::warn!("Failed to load locale_list.txt due to {}", err);
        }
        product_info
    }
}

impl Into<fidl_fuchsia_hwinfo::ProductInfo> for ProductInfo {
    fn into(self) -> fidl_fuchsia_hwinfo::ProductInfo {
        let mut locale_list: Vec<LocaleId> = Vec::new();
        for locale in self.locales {
            locale_list.push(LocaleId { id: locale });
        }
        fidl_fuchsia_hwinfo::ProductInfo {
            sku: self.sku,
            language: self.language,
            regulatory_domain: if self.country_code.is_none() {
                None
            } else {
                Some(RegulatoryDomain { country_code: self.country_code, ..Default::default() })
            },
            locale_list: if locale_list.is_empty() { None } else { Some(locale_list) },
            name: self.name,
            model: self.model,
            manufacturer: self.manufacturer,
            build_date: self.build_date,
            build_name: self.build_name,
            colorway: self.colorway,
            display: self.display,
            memory: self.memory,
            nand_storage: self.nand_storage,
            emmc_storage: self.emmc_storage,
            microphone: self.microphone,
            audio_amplifier: self.audio_amplifier,
            ..Default::default()
        }
    }
}
