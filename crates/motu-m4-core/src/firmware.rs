use crate::descriptor;

pub const MAX_FIRMWARE_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareError {
    Length,
    Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareImage<'a> {
    version: &'a str,
    payload: &'a [u8],
    stack: u32,
    reset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbDescriptorCandidate<'a> {
    offset: usize,
    bytes: &'a [u8],
}

impl<'a> UsbDescriptorCandidate<'a> {
    pub(crate) fn from_payload(payload: &'a [u8], offset: usize, length: usize) -> Option<Self> {
        Some(Self {
            offset: offset.checked_add(64)?,
            bytes: payload.get(offset..offset.checked_add(length)?)?,
        })
    }

    pub const fn offset(self) -> usize {
        self.offset
    }
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}

impl<'a> FirmwareImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FirmwareError> {
        let declared = u32::from_le_bytes(*bytes.first_chunk().ok_or(FirmwareError::Length)?);
        payload_length(bytes.len(), declared)?;
        let (header, payload) = bytes.split_at(64);
        let version_bytes: &[u8; 60] =
            header[4..].try_into().map_err(|_| FirmwareError::Version)?;
        let end = version_length(version_bytes)?;
        let version =
            core::str::from_utf8(&version_bytes[..end]).map_err(|_| FirmwareError::Version)?;
        Ok(Self {
            version,
            payload,
            stack: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
            reset: u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
        })
    }

    pub const fn version(self) -> &'a str {
        self.version
    }
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }
    pub const fn initial_stack_pointer(self) -> u32 {
        self.stack
    }
    pub const fn reset_handler(self) -> u32 {
        self.reset
    }

    pub fn usb_devices(self) -> impl Iterator<Item = UsbDescriptorCandidate<'a>> {
        self.payload
            .windows(18)
            .enumerate()
            .filter_map(move |(offset, _)| usb_device_at(self.payload, offset))
    }

    pub fn usb_configurations(self) -> impl Iterator<Item = UsbDescriptorCandidate<'a>> {
        self.payload
            .windows(9)
            .enumerate()
            .filter_map(move |(offset, _)| usb_configuration_at(self.payload, offset))
    }
}

pub(crate) fn payload_length(total: usize, declared: u32) -> Result<(), FirmwareError> {
    if !(72..=MAX_FIRMWARE_BYTES).contains(&total)
        || usize::try_from(declared).ok() != total.checked_sub(64)
    {
        return Err(FirmwareError::Length);
    }
    Ok(())
}

pub(crate) fn version_length(bytes: &[u8; 60]) -> Result<usize, FirmwareError> {
    let mut end = None;
    for (index, byte) in bytes.iter().enumerate() {
        match (end, *byte) {
            (None, 0) if index > 0 => end = Some(index),
            (None, 32..=126) | (Some(_), 0) => {}
            _ => return Err(FirmwareError::Version),
        }
    }
    end.ok_or(FirmwareError::Version)
}

pub(crate) fn usb_device_at(payload: &[u8], offset: usize) -> Option<UsbDescriptorCandidate<'_>> {
    let candidate = UsbDescriptorCandidate::from_payload(payload, offset, 18)?;
    let bytes = candidate.bytes();
    if bytes[..4] != [18, 1, 0, 2] || bytes[7] != 64 || bytes[8..10] != [0xfd, 7] || bytes[17] != 1
    {
        return None;
    }
    Some(candidate)
}

pub(crate) fn usb_configuration_at(
    payload: &[u8],
    offset: usize,
) -> Option<UsbDescriptorCandidate<'_>> {
    let header = payload.get(offset..offset.checked_add(9)?)?;
    if header[..2] != [9, 2] || header[4] == 0 || header[5] != 1 {
        return None;
    }
    let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
    let candidate = UsbDescriptorCandidate::from_payload(payload, offset, length)?;
    if length < 9 || !descriptor_chain(candidate.bytes()) {
        return None;
    }
    Some(candidate)
}

fn descriptor_chain(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let Ok((_, rest)) = descriptor(bytes) else {
            return false;
        };
        bytes = rest;
    }
    true
}
