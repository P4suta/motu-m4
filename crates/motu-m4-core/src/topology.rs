use crate::{field, word, Error};

#[derive(Clone, Copy)]
struct Terminal {
    id: u8,
    clock: u8,
    source: u8,
}

#[derive(Default)]
pub(crate) struct AudioTopology {
    total: usize,
    length: Option<u16>,
    usb_input: Option<Terminal>,
    usb_output: Option<Terminal>,
    external_input: Option<Terminal>,
    external_output: Option<Terminal>,
}

impl AudioTopology {
    pub(crate) fn add(&mut self, descriptor: &[u8]) -> Result<(), Error> {
        self.total = self
            .total
            .checked_add(descriptor.len())
            .ok_or(Error::Layout)?;
        match field(descriptor, 2)? {
            1 => {
                if descriptor.len() != 9
                    || word(descriptor, 3)? != 0x0200
                    || self.length.replace(word(descriptor, 6)?).is_some()
                {
                    return Err(Error::Layout);
                }
            }
            2 | 3 => {
                let input = field(descriptor, 2)? == 2;
                let expected = if input { 17 } else { 12 };
                if descriptor.len() != expected || (input && field(descriptor, 8)? != 4) {
                    return Err(Error::Layout);
                }
                let terminal = Terminal {
                    id: field(descriptor, 3)?,
                    clock: field(descriptor, if input { 7 } else { 8 })?,
                    source: if input { 0 } else { field(descriptor, 7)? },
                };
                let slot = match (word(descriptor, 4)?, input) {
                    (0x0101, true) => &mut self.usb_input,
                    (0x0101, false) => &mut self.usb_output,
                    (0x0602, true) => &mut self.external_input,
                    (0x0602, false) => &mut self.external_output,
                    _ => return Err(Error::Layout),
                };
                if slot.replace(terminal).is_some() {
                    return Err(Error::Layout);
                }
            }
            10 | 11 => {}
            _ => return Err(Error::Layout),
        }
        Ok(())
    }

    pub(crate) fn check(
        &self,
        clock: u8,
        selector: u8,
        playback: u8,
        capture: u8,
    ) -> Result<(), Error> {
        let usb_input = self.usb_input.ok_or(Error::Layout)?;
        let usb_output = self.usb_output.ok_or(Error::Layout)?;
        let external_input = self.external_input.ok_or(Error::Layout)?;
        let external_output = self.external_output.ok_or(Error::Layout)?;
        if self.length.map(usize::from) != Some(self.total)
            || usb_input.id != playback
            || usb_output.id != capture
            || usb_output.source != external_input.id
            || external_output.source != usb_input.id
        {
            return Err(Error::Layout);
        }
        for terminal in [usb_input, usb_output, external_input, external_output] {
            if terminal.clock != selector {
                return Err(Error::Layout);
            }
        }
        let ids = [
            clock,
            selector,
            usb_input.id,
            usb_output.id,
            external_input.id,
            external_output.id,
        ];
        for (index, id) in ids.iter().enumerate() {
            if *id == 0 || ids.get(..index).ok_or(Error::Layout)?.contains(id) {
                return Err(Error::Layout);
            }
        }
        Ok(())
    }
}
