use crate::{Budget, ControlError, Machine, MemoryLayout, Peripheral, Stop};
use motu_m4_core::{FirmwareImage, MAX_FIRMWARE_BYTES};
use sha2::{Digest, Sha256};

const DIGEST: [u8; 32] = [
    0x25, 0xd7, 0xfa, 0xf9, 0x30, 0xca, 0x29, 0x1a, 0xed, 0x83, 0xc0, 0xdf, 0xfa, 0xd9, 0x17, 0xef,
    0x67, 0x20, 0x50, 0xf6, 0xac, 0xa6, 0x71, 0x1f, 0x66, 0x6b, 0x37, 0x99, 0x6e, 0x1e, 0xc8, 0x6b,
];

pub struct AtmelAudioControl {
    machine: Machine,
    instructions: u64,
    packets: Vec<Vec<u8>>,
}

impl AtmelAudioControl {
    pub fn new(bytes: &[u8]) -> Result<Self, ControlError> {
        if bytes.len() > MAX_FIRMWARE_BYTES || Sha256::digest(bytes).as_slice() != DIGEST {
            return Err(ControlError::Identity);
        }
        let image = FirmwareImage::parse(bytes).map_err(|_| ControlError::Identity)?;
        let mut machine = Machine::new(image, MemoryLayout::new(0x2040_0000, 0x60000)?)?;
        machine.attach(Peripheral::SamUsbControl)?;
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

    fn interrupt(&mut self) -> Result<u32, ControlError> {
        self.machine.prepare_call(0x2040_1c09, [0; 4])?;
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
        self.machine.sam_receive(&setup, true)?;
        self.interrupt()?;
        if setup[0] == 0x21 {
            if self.machine.sam_begin_in()? {
                self.interrupt()?;
                self.machine.sam_packet()?;
            }
            self.machine.sam_receive(data, false)?;
            if self.interrupt()? != 1 {
                return Err(ControlError::Stall);
            }
            return Ok(length);
        }
        let mut offset = 0;
        for _ in 0..17 {
            if !self.machine.sam_begin_in()? {
                return Ok(offset);
            }
            self.interrupt()?;
            let packet = self.machine.sam_packet()?.ok_or(ControlError::Descriptor)?;
            let remaining = data.get_mut(offset..).ok_or(ControlError::Descriptor)?;
            let target = remaining
                .get_mut(..packet.len())
                .ok_or(ControlError::Descriptor)?;
            target.copy_from_slice(&packet);
            offset += packet.len();
            self.packets.push(packet);
        }
        Err(ControlError::Descriptor)
    }
}
