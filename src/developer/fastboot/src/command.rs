// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;
use thiserror::Error;

#[derive(PartialEq, Debug, Clone)]
pub enum ClientVariable {
    // All variables.
    All,
    // Version of FastBoot protocol supported. It should be "0.4".
    Version,
    // Version string for the Bootloader.
    VersionBootLoader,
    // Version string of the Baseband Software.
    VersionBaseBand,
    // Name of the product.
    Product,
    // Product serial number.
    SerialNumber,
    // If the value is "yes", this is a secure bootloader requiring a signature before
    // it will install or boot images.
    Secure,
    // If the value is "yes", the device is running fastbootd. Otherwise, it is running fastboot
    // in the bootloader.
    IsUserSpace,
    // OEM custom variables
    Oem(String),
}

#[derive(PartialEq, Debug, Clone)]
pub enum Command {
    // Read a config/version variable from the bootloader.  The variable contents will be returned
    // after the OKAY response. If the variable is unknown, the bootloader should return a FAIL
    // response, optionally with an error message.
    GetVar(ClientVariable),
    // Write data to memory which will be later used by "boot", "ramdisk", "flash", etc.  The
    // client will reply with "DATA%08x" if it has enough space in RAM or "FAIL" if not.  The size
    // of the download is remembered.
    Download(u32),
    // Read data from memory which was staged by the last command, e.g. an oem command.  The client
    // will reply with "DATA%08x" if it is ready to send %08x bytes of data.  If no data was staged
    // in the last command, the client must reply with "FAIL".  After the client successfully sends
    // %08x bytes, the client shall send a single packet starting with "OKAY".  Clients should not
    // support "upload" unless it supports an oem command that requires "upload" capabilities.
    Upload,
    // Write the previously downloaded image to the named partition (if possible).
    Flash(String),
    // Erase the indicated partition (clear to 0xFFs).
    Erase(String),
    // The previously downloaded data is a boot.img and should be booted according to the normal
    // procedure for a boot.img.
    Boot,
    // Continue booting as normal (if possible).
    Continue,
    // Reboot the device.
    Reboot,
    // Reboot back into the bootloader. Useful for upgrade processes that require upgrading
    // the bootloader and then upgrading other partitions using the new bootloader.
    RebootBootLoader,
    // Set the active slot.
    SetActive(String),

    ////////////// Logical Partition Commands ////////////////////////////////////////////
    //
    // Write the previously downloaded image to a super partition. Unlike the "flash" command, this
    // has special rules. The image must have been created by the lpmake command, and must not be a
    // sparse image.  If the last argument is "wipe", then all existing logical partitions are
    // deleted. If no final argument is specified, the partition tables are merged. Any partition
    // in the new image that does not exist in the old image is created with a zero size.
    //
    // In all cases, this will cause the temporary "scratch" partition to be deleted if it exists.
    UpdateSuper(String, String),
    // Create a logical partition with the given name and size, in the super partition.
    CreateLogicalPartition(String, usize),
    // Delete a logical partition with the given name.
    DeleteLogicalPartition(String),
    // Change the size of the named logical partition.
    ResizeLogicalPartition(String, usize),
    // If the value is "yes", the partition is logical. Otherwise the partition is physical.
    IsLogical(String),

    // Combination of partition name, partition offset, downloaded data checksum.
    StreamFlash { partition: String, offset_bytes: u64, crc32: u32 },
    // Combination of partition name, partition offset, payload length in bytes, payload value.
    StreamFill { partition: String, offset_bytes: u64, length_bytes: u64, val: u32 },
    ////////////// OEM Commands ////////////////////////////////////////////
    //
    // Support for OEM commands not specifically defined in the Fastboot specification.
    Oem(String),
}

/// The maximum length for outgoing fastboot commands to the device.
pub const MAX_COMMAND_LENGTH: usize = 64;

#[derive(Debug, Error)]
#[error("Client Variable name is too long: {}", client_variable)]
pub struct ClientVariableTooLongError {
    client_variable: String,
}

#[derive(Debug, Error)]
pub enum SerializeCommandError {
    #[error("Message name is too long for command.")]
    MessageNameTooLong {},
    #[error("GetVar command malformed.")]
    InvalidGetVarCommand(#[from] ClientVariableTooLongError),
}

impl TryFrom<&ClientVariable> for Vec<u8> {
    type Error = ClientVariableTooLongError;

    fn try_from(var: &ClientVariable) -> Result<Self, Self::Error> {
        match var {
            ClientVariable::All => Ok(b"all".to_vec()),
            ClientVariable::Version => Ok(b"version".to_vec()),
            ClientVariable::VersionBootLoader => Ok(b"version-bootloader".to_vec()),
            ClientVariable::VersionBaseBand => Ok(b"version-baseband".to_vec()),
            ClientVariable::Product => Ok(b"product".to_vec()),
            ClientVariable::SerialNumber => Ok(b"serialno".to_vec()),
            ClientVariable::Secure => Ok(b"secure".to_vec()),
            ClientVariable::IsUserSpace => Ok(b"is-userspace".to_vec()),
            ClientVariable::Oem(s) => {
                let bytes = s.as_bytes();
                // Need to make sure it won't break the "getvar:" command
                if bytes.len() > (MAX_COMMAND_LENGTH - 7) {
                    Err(ClientVariableTooLongError { client_variable: s.to_string() })
                } else {
                    Ok(bytes.to_vec())
                }
            }
        }
    }
}

fn concat_message(cmd: &[u8], s: &str) -> Result<Vec<u8>, SerializeCommandError> {
    let bytes = s.as_bytes();
    if MAX_COMMAND_LENGTH - cmd.len() < bytes.len() {
        Err(SerializeCommandError::MessageNameTooLong {})
    } else {
        Ok([cmd, &bytes[..]].concat())
    }
}

impl TryFrom<&Command> for Vec<u8> {
    type Error = SerializeCommandError;

    fn try_from(command: &Command) -> Result<Self, Self::Error> {
        match command {
            Command::GetVar(v) => Ok([b"getvar:", &Vec::<u8>::try_from(v)?[..]].concat()),
            Command::Download(s) => Ok([b"download:", format!("{:08x}", s).as_bytes()].concat()),
            Command::Upload => Ok(b"upload".to_vec()),
            Command::Flash(s) => concat_message(b"flash:", s),
            Command::Erase(s) => concat_message(b"erase:", s),
            Command::Boot => Ok(b"boot".to_vec()),
            Command::Continue => Ok(b"continue".to_vec()),
            Command::Reboot => Ok(b"reboot".to_vec()),
            Command::RebootBootLoader => Ok(b"reboot-bootloader".to_vec()),
            Command::UpdateSuper(partition_name, arg) => {
                concat_message(b"update-super:", &format!("{}:{}", partition_name, arg))
            }
            Command::CreateLogicalPartition(partition_name, size) => concat_message(
                b"create-logical-partition:",
                &format!("{}:{}", partition_name, size),
            ),
            Command::DeleteLogicalPartition(s) => concat_message(b"delete-logical-partition:", s),
            Command::ResizeLogicalPartition(partition_name, size) => concat_message(
                b"resize-logical-partition:",
                &format!("{}:{}", partition_name, size),
            ),
            Command::IsLogical(s) => concat_message(b"is-logical:", s),
            Command::SetActive(s) => concat_message(b"set_active:", s),
            Command::Oem(s) => concat_message(b"oem ", s),
            Command::StreamFlash { partition, offset_bytes, crc32 } => concat_message(
                b"stream-flash:",
                &format!("{}:{:X}:{:X}", partition, offset_bytes, crc32),
            ),
            Command::StreamFill { partition, offset_bytes, length_bytes, val } => concat_message(
                b"stream-fill:",
                &format!("{}:{:X}:{:X}:{:X}", partition, offset_bytes, length_bytes, val),
            ),
        }
    }
}

fn write_command(reply: &Command, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match reply {
        Command::GetVar(v) => write!(f, "GET_VAR: {}", v),
        Command::Download(s) => write!(f, "DOWNLOAD: {}", s),
        Command::Upload => write!(f, "UPLOAD"),
        Command::Flash(s) => write!(f, "FLASH: {}", s),
        Command::Erase(s) => write!(f, "ERASE: {}", s),
        Command::Boot => write!(f, "BOOT"),
        Command::Continue => write!(f, "CONTINUE"),
        Command::Reboot => write!(f, "REBOOT"),
        Command::RebootBootLoader => write!(f, "REBOOT_BOOTLOADER"),
        Command::UpdateSuper(partition_name, arg) => {
            write!(f, "UPDATE_SUPER: {}:{}", partition_name, arg)
        }
        Command::CreateLogicalPartition(partition_name, size) => {
            write!(f, "CREATE_LOGICAL_PARTITION: {}:{}", partition_name, size)
        }
        Command::DeleteLogicalPartition(s) => write!(f, "DELETE-LOGICAL-PARTITION: {}", s),
        Command::ResizeLogicalPartition(partition_name, size) => {
            write!(f, "RESIZE-LOGICAL-PARTITION: {}:{}", partition_name, size)
        }
        Command::IsLogical(s) => write!(f, "IS_LOGICAL: {}", s),
        Command::SetActive(s) => write!(f, "SET_ACTIVE: {}", s),
        Command::Oem(s) => write!(f, "OEM {}", s),
        Command::StreamFlash { partition, offset_bytes, crc32 } => {
            write!(f, "STREAM-FLASH: {}:{:X}:{:X}", partition, offset_bytes, crc32)
        }
        Command::StreamFill { partition, offset_bytes, length_bytes, val } => {
            write!(f, "STREAM-FILL: {}:{:X}:{:X}:{:X}", partition, offset_bytes, length_bytes, val)
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_command(self, f)
    }
}

fn write_client_variable(variable: &ClientVariable, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match variable {
        ClientVariable::All => write!(f, "ALL"),
        ClientVariable::Version => write!(f, "VERSION"),
        ClientVariable::VersionBootLoader => write!(f, "VERSION_BOOTLOADER"),
        ClientVariable::VersionBaseBand => write!(f, "VERSION_BASEBAND"),
        ClientVariable::Product => write!(f, "PRODUCT"),
        ClientVariable::SerialNumber => write!(f, "SERIAL_NUMBER"),
        ClientVariable::Secure => write!(f, "SECURE"),
        ClientVariable::IsUserSpace => write!(f, "IS_USERSPACE"),
        ClientVariable::Oem(s) => write!(f, "OEM: {}", s),
    }
}

impl fmt::Display for ClientVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_client_variable(self, f)
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_var() {
        let version = Vec::<u8>::try_from(&Command::GetVar(ClientVariable::Version));
        assert!(!version.is_err());
        assert_eq!(version.unwrap(), b"getvar:version".to_vec());

        let version_boot_loader =
            Vec::<u8>::try_from(&Command::GetVar(ClientVariable::VersionBootLoader));
        assert!(!version_boot_loader.is_err());
        assert_eq!(version_boot_loader.unwrap(), b"getvar:version-bootloader".to_vec());

        let version_base_band =
            Vec::<u8>::try_from(&Command::GetVar(ClientVariable::VersionBaseBand));
        assert!(!version_base_band.is_err());
        assert_eq!(version_base_band.unwrap(), b"getvar:version-baseband".to_vec());

        let product = Vec::<u8>::try_from(&Command::GetVar(ClientVariable::Product));
        assert!(!product.is_err());
        assert_eq!(product.unwrap(), b"getvar:product".to_vec());

        let serial_number = Vec::<u8>::try_from(&Command::GetVar(ClientVariable::SerialNumber));
        assert!(!serial_number.is_err());
        assert_eq!(serial_number.unwrap(), b"getvar:serialno".to_vec());

        let secure = Vec::<u8>::try_from(&Command::GetVar(ClientVariable::Secure));
        assert!(!secure.is_err());
        assert_eq!(secure.unwrap(), b"getvar:secure".to_vec());

        let is_user_space = Vec::<u8>::try_from(&Command::GetVar(ClientVariable::IsUserSpace));
        assert!(!is_user_space.is_err());
        assert_eq!(is_user_space.unwrap(), b"getvar:is-userspace".to_vec());
    }

    #[test]
    fn test_download() {
        let download = Vec::<u8>::try_from(&Command::Download(u32::MIN));
        assert!(!download.is_err());
        assert_eq!(download.unwrap(), b"download:00000000".to_vec());

        let download_max = Vec::<u8>::try_from(&Command::Download(u32::MAX));
        assert!(!download_max.is_err());
        assert_eq!(download_max.unwrap(), b"download:ffffffff".to_vec());

        // Test something in between.
        let download_fifteen = Vec::<u8>::try_from(&Command::Download(15));
        assert!(!download_fifteen.is_err());
        assert_eq!(download_fifteen.unwrap(), b"download:0000000f".to_vec());
    }

    #[test]
    fn test_upload() {
        let byte_vector = Vec::<u8>::try_from(&Command::Upload);
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"upload".to_vec());
    }

    #[test]
    fn test_flash() {
        let byte_vector = Vec::<u8>::try_from(&Command::Flash("test".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"flash:test".to_vec());

        let max_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"flash:".len()];
        let max_vector =
            Vec::<u8>::try_from(&Command::Flash(String::from_utf8(max_partition_name).unwrap()));
        assert!(!max_vector.is_err());

        let over_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"flash:".len() + 1];
        let over_vector =
            Vec::<u8>::try_from(&Command::Flash(String::from_utf8(over_partition_name).unwrap()));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_erase() {
        let byte_vector = Vec::<u8>::try_from(&Command::Erase("test".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"erase:test".to_vec());

        let max_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"erase:".len()];
        let max_vector =
            Vec::<u8>::try_from(&Command::Erase(String::from_utf8(max_partition_name).unwrap()));
        assert!(!max_vector.is_err());

        let over_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"erase:".len() + 1];
        let over_vector =
            Vec::<u8>::try_from(&Command::Erase(String::from_utf8(over_partition_name).unwrap()));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_boot() {
        let byte_vector = Vec::<u8>::try_from(&Command::Boot);
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"boot".to_vec());
    }

    #[test]
    fn test_continue() {
        let byte_vector = Vec::<u8>::try_from(&Command::Continue);
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"continue".to_vec());
    }

    #[test]
    fn test_reboot() {
        let byte_vector = Vec::<u8>::try_from(&Command::Reboot);
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"reboot".to_vec());
    }

    #[test]
    fn test_reboot_bootloader() {
        let byte_vector = Vec::<u8>::try_from(&Command::RebootBootLoader);
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"reboot-bootloader".to_vec());
    }

    #[test]
    fn test_update_super() {
        let byte_vector =
            Vec::<u8>::try_from(&Command::UpdateSuper("test".to_string(), "test2".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"update-super:test:test2".to_vec());

        let max_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"update-super:".len() - 1];
        let max_vector = Vec::<u8>::try_from(&Command::UpdateSuper(
            String::from_utf8(max_partition_name).unwrap(),
            "".to_string(),
        ));
        assert!(!max_vector.is_err());

        let over_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"update-super:".len()];
        let over_vector = Vec::<u8>::try_from(&Command::UpdateSuper(
            String::from_utf8(over_partition_name).unwrap(),
            "".to_string(),
        ));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_create_logical_partition() {
        let byte_vector =
            Vec::<u8>::try_from(&Command::CreateLogicalPartition("test".to_string(), 5));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"create-logical-partition:test:5".to_vec());

        let max_partition_name =
            vec![b'X'; MAX_COMMAND_LENGTH - b"create-logical-partition:".len() - 2];
        let max_vector = Vec::<u8>::try_from(&Command::CreateLogicalPartition(
            String::from_utf8(max_partition_name).unwrap(),
            0,
        ));
        assert!(!max_vector.is_err());

        let over_partition_name =
            vec![b'X'; MAX_COMMAND_LENGTH - b"create-logical-partition:".len()];
        let over_vector = Vec::<u8>::try_from(&Command::CreateLogicalPartition(
            String::from_utf8(over_partition_name).unwrap(),
            0,
        ));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_delete_logical_partition() {
        let byte_vector = Vec::<u8>::try_from(&Command::DeleteLogicalPartition("test".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"delete-logical-partition:test".to_vec());

        let max_partition_name =
            vec![b'X'; MAX_COMMAND_LENGTH - b"delete-logical-partition:".len()];
        let max_vector = Vec::<u8>::try_from(&Command::DeleteLogicalPartition(
            String::from_utf8(max_partition_name).unwrap(),
        ));
        assert!(!max_vector.is_err());

        let over_partition_name =
            vec![b'X'; MAX_COMMAND_LENGTH - b"delete-logical-partition:".len() + 1];
        let over_vector = Vec::<u8>::try_from(&Command::DeleteLogicalPartition(
            String::from_utf8(over_partition_name).unwrap(),
        ));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_resize_logical_partition() {
        let byte_vector =
            Vec::<u8>::try_from(&Command::ResizeLogicalPartition("test".to_string(), 5));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"resize-logical-partition:test:5".to_vec());

        let max_partition_name =
            vec![b'X'; MAX_COMMAND_LENGTH - b"resize-logical-partition:".len() - 2];
        let max_vector = Vec::<u8>::try_from(&Command::ResizeLogicalPartition(
            String::from_utf8(max_partition_name).unwrap(),
            0,
        ));
        assert!(!max_vector.is_err());

        let over_partition_name =
            vec![b'X'; MAX_COMMAND_LENGTH - b"resize-logical-partition:".len()];
        let over_vector = Vec::<u8>::try_from(&Command::ResizeLogicalPartition(
            String::from_utf8(over_partition_name).unwrap(),
            0,
        ));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_is_logical() {
        let byte_vector = Vec::<u8>::try_from(&Command::IsLogical("test".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"is-logical:test".to_vec());

        let max_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"is-logical:".len()];
        let max_vector = Vec::<u8>::try_from(&Command::IsLogical(
            String::from_utf8(max_partition_name).unwrap(),
        ));
        assert!(!max_vector.is_err());

        let over_partition_name = vec![b'X'; MAX_COMMAND_LENGTH - b"is-logical:".len() + 1];
        let over_vector = Vec::<u8>::try_from(&Command::IsLogical(
            String::from_utf8(over_partition_name).unwrap(),
        ));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_set_active() {
        let byte_vector = Vec::<u8>::try_from(&Command::SetActive("a".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"set_active:a".to_vec());

        let max_slot_name = vec![b'X'; MAX_COMMAND_LENGTH - b"set_active:".len()];
        let max_vector =
            Vec::<u8>::try_from(&Command::SetActive(String::from_utf8(max_slot_name).unwrap()));
        assert!(!max_vector.is_err());

        let over_slot_name = vec![b'X'; MAX_COMMAND_LENGTH - b"set_active:".len() + 1];
        let over_vector =
            Vec::<u8>::try_from(&Command::SetActive(String::from_utf8(over_slot_name).unwrap()));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_oem_variables() {
        let oem_variable_vector = Vec::<u8>::try_from(&ClientVariable::Oem("test".to_string()));
        assert!(!oem_variable_vector.is_err());
        assert_eq!(oem_variable_vector.unwrap(), b"test".to_vec());

        let max_variable_name =
            String::from_utf8(vec![b'X'; MAX_COMMAND_LENGTH - b"getvar:".len()]).unwrap();
        let max_vector = Vec::<u8>::try_from(&ClientVariable::Oem(max_variable_name));
        assert!(!max_vector.is_err());

        let over_variable_name =
            String::from_utf8(vec![b'X'; MAX_COMMAND_LENGTH - b"getvar:".len() + 1]).unwrap();
        let over_vector = Vec::<u8>::try_from(&ClientVariable::Oem(over_variable_name));
        assert!(over_vector.is_err());
    }

    #[test]
    fn test_oem_command() {
        let byte_vector = Vec::<u8>::try_from(&Command::Oem("test".to_string()));
        assert!(!byte_vector.is_err());
        assert_eq!(byte_vector.unwrap(), b"oem test".to_vec());

        let max_command_name = String::from_utf8(vec![b'X'; MAX_COMMAND_LENGTH - 7]).unwrap();
        let max_command = Vec::<u8>::try_from(&Command::Oem(max_command_name));
        assert!(!max_command.is_err());

        let over_command_name = String::from_utf8(vec![b'X'; MAX_COMMAND_LENGTH + 1]).unwrap();
        let over_command = Vec::<u8>::try_from(&Command::Oem(over_command_name));
        assert!(over_command.is_err());
    }
}
