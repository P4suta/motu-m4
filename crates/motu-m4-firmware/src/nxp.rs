use crate::{Budget, Error, Execution, Machine, MemoryLayout, Peripheral, Stop};
use motu_m4_core::{FirmwareImage, MAX_FIRMWARE_BYTES};
use sha2::{Digest, Sha256};

const GET: u32 = 0x2000_bf49;
const SET: u32 = 0x2000_c0f5;
const SETUP: u32 = 0x4010_0080;
const BUFFER: u32 = 0x4010_0100;
const IN_DESCRIPTOR: u32 = 0x4010_0008;
const PROGRESS: u32 = 0x2002_a5b4;
const OUT_STAGE: u32 = 0x2002_a5e0;
const DIGEST: [u8; 32] = [
    0x80, 0xa7, 0xe4, 0x79, 0xf1, 0x5c, 0x5f, 0x2d, 0xf8, 0xb5, 0xd6, 0x6e, 0x9f, 0x35, 0x5d, 0x93,
    0x84, 0x1d, 0x38, 0xf9, 0x2d, 0xdd, 0x7e, 0x9e, 0x6e, 0x05, 0xfc, 0xc6, 0x6e, 0x0f, 0x57, 0x48,
];

struct InTransfer {
    address: u32,
    bytes: usize,
    more: bool,
}

impl InTransfer {
    fn decode(word: u32, more: u32, remaining: usize) -> Result<Self, ControlError> {
        if word & 0x8000_0000 == 0 {
            return Err(ControlError::Stall);
        }
        let bytes = ((word >> 11) & 0x7fff) as usize;
        if bytes > 64 || bytes > remaining || more > 1 || (more == 1 && bytes == 0) {
            return Err(ControlError::Descriptor);
        }
        let address = 0x4010_0000 + ((word & 0x7ff) << 6);
        Ok(Self {
            address,
            bytes,
            more: more == 1,
        })
    }
}

#[derive(Debug)]
pub enum ControlError {
    Identity,
    Request,
    Stall,
    Descriptor,
    Machine(Error),
    Execution(Box<Execution>),
    Mismatch {
        setup: [u8; 8],
        expected: Vec<u8>,
        actual: Vec<u8>,
        expected_count: usize,
        actual_count: usize,
    },
}

impl From<Error> for ControlError {
    fn from(error: Error) -> Self {
        Self::Machine(error)
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity => f.write_str("firmware does not match a supported SHA-256 digest"),
            Self::Request => f.write_str("control setup does not match its buffer or endpoint limits"),
            Self::Stall => f.write_str("firmware rejected the control request"),
            Self::Descriptor => f.write_str("firmware returned an invalid endpoint response"),
            Self::Machine(error) => write!(f, "firmware memory or execution state: {error:?}"),
            Self::Execution(result) => write!(f, "firmware stopped: {:?}; pc={:#010x}, instructions={}, trace={:x?}", result.stop, result.registers[15], result.instructions, result.trace),
            Self::Mismatch { setup, expected, actual, expected_count, actual_count } => write!(f, "firmware response mismatch: setup={setup:02x?}, expected={expected_count} bytes {expected:02x?}, actual={actual_count} bytes {actual:02x?}"),
        }
    }
}

impl std::error::Error for ControlError {}

pub struct NxpAudioControl {
    machine: Machine,
    instructions: u64,
    packets: Vec<Vec<u8>>,
}

impl NxpAudioControl {
    pub fn new(bytes: &[u8]) -> Result<Self, ControlError> {
        if bytes.len() > MAX_FIRMWARE_BYTES || Sha256::digest(bytes).as_slice() != DIGEST {
            return Err(ControlError::Identity);
        }
        let image = FirmwareImage::parse(bytes).map_err(|_| ControlError::Identity)?;
        let mut machine = Machine::new(image, MemoryLayout::new(0x2000_0000, 0x30000)?)?;
        machine.attach(Peripheral::NxpUsbSram)?;
        Ok(Self {
            machine,
            instructions: 0,
            packets: Vec::new(),
        })
    }

    pub fn instructions(&self) -> u64 {
        self.instructions
    }
    pub fn packets(&self) -> &[Vec<u8>] {
        &self.packets
    }

    fn invoke(&mut self, entry: u32) -> Result<u32, ControlError> {
        self.machine.prepare_call(entry, [0; 4])?;
        let result = self.machine.run(Budget::new(10_000)?)?;
        self.instructions += result.instructions;
        if result.stop != Stop::Returned {
            return Err(ControlError::Execution(Box::new(result)));
        }
        Ok(result.registers[0])
    }

    pub fn control(&mut self, setup: [u8; 8], data: &mut [u8]) -> Result<usize, ControlError> {
        let length = usize::from(u16::from_le_bytes([setup[6], setup[7]]));
        if length != data.len()
            || length > 1024
            || !matches!(setup[0], 0xa1 | 0x21)
            || (setup[0] == 0x21 && length > 64)
        {
            return Err(ControlError::Request);
        }
        self.packets.clear();
        self.machine.write(SETUP, &setup)?;
        self.machine.write(PROGRESS, &0_u32.to_le_bytes())?;
        if setup[0] == 0x21 {
            self.machine.write(OUT_STAGE, &0_u32.to_le_bytes())?;
            let initial = self.invoke(SET)?;
            if initial != 0 {
                return Err(ControlError::Descriptor);
            }
            self.machine.write(PROGRESS + 4, &BUFFER.to_le_bytes())?;
            self.machine.write(BUFFER, data)?;
            if self.invoke(SET)? != 1 {
                return Err(ControlError::Stall);
            }
            return Ok(data.len());
        }
        let mut offset = 0;
        for _ in 0..17 {
            self.machine.write(IN_DESCRIPTOR, &0_u32.to_le_bytes())?;
            let more = self.invoke(GET)?;
            let word = self.machine.read(IN_DESCRIPTOR, 4)?;
            let word = u32::from_le_bytes(word.try_into().map_err(|_| ControlError::Descriptor)?);
            let remaining = data.get_mut(offset..).ok_or(ControlError::Descriptor)?;
            let transfer = InTransfer::decode(word, more, remaining.len())?;
            let count = transfer.bytes;
            let packet = self.machine.read(transfer.address, count)?;
            remaining[..count].copy_from_slice(&packet);
            offset += count;
            self.packets.push(packet);
            if !transfer.more {
                return Ok(offset);
            }
        }
        Err(ControlError::Descriptor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_descriptor_requires_ownership_capacity_and_finite_progress() {
        let transfer = InTransfer::decode(0x80020004, 1, 64).unwrap();
        assert_eq!(transfer.address, 0x40100100);
        assert_eq!(transfer.bytes, 64);
        assert!(transfer.more);
        let empty = InTransfer::decode(0x80000004, 0, 0).unwrap();
        assert_eq!(empty.bytes, 0);
        assert!(!empty.more);
        for word in [0, 4, 0x40020004] {
            assert!(matches!(
                InTransfer::decode(word, 0, 64),
                Err(ControlError::Stall)
            ));
        }
        for (word, more, remaining) in [
            (0x80020004, 0, 63),
            (0x80020804, 0, 65),
            (0x80020004, 2, 64),
            (0x80000004, 1, 64),
        ] {
            assert!(matches!(
                InTransfer::decode(word, more, remaining),
                Err(ControlError::Descriptor)
            ));
        }
    }
}
