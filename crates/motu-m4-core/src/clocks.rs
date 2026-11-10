use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRate(u32);

impl SampleRate {
    pub fn new(hz: u32) -> Result<Self, Error> {
        match hz {
            44_100 | 48_000 | 88_200 | 96_000 | 176_400 | 192_000 => Ok(Self(hz)),
            _ => Err(Error::Clock),
        }
    }

    pub const fn hz(self) -> u32 {
        self.0
    }

    pub const fn max_packet_bytes(self) -> u32 {
        (self.0.div_ceil(8000) + 1) * 16
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockRangeCount(u16);

impl ClockRangeCount {
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let header = <[u8; 2]>::try_from(data).map_err(|_| Error::Clock)?;
        let count = u16::from_le_bytes(header);
        if !(1..=32).contains(&count) {
            return Err(Error::Clock);
        }
        Ok(Self(count))
    }

    pub const fn bytes(self) -> u16 {
        2 + self.0 * 12
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateRange {
    min: u32,
    max: u32,
    resolution: u32,
}

impl RateRange {
    pub fn new(min: u32, max: u32, resolution: u32) -> Result<Self, Error> {
        if min == 0 || min > max || (min == max) != (resolution == 0) {
            return Err(Error::Clock);
        }
        Ok(Self {
            min,
            max,
            resolution,
        })
    }

    pub fn contains(self, hz: u32) -> bool {
        match self.offset(hz) {
            Some(offset) => self.resolution == 0 || offset % self.resolution == 0,
            None => false,
        }
    }

    pub(crate) fn offset(self, hz: u32) -> Option<u32> {
        if hz > self.max {
            return None;
        }
        hz.checked_sub(self.min)
    }

    fn parse(data: &[u8]) -> Result<Self, Error> {
        Self::new(value(data, 0)?, value(data, 4)?, value(data, 8)?)
    }
}

#[derive(Debug)]
pub struct ClockRanges<'a>(&'a [u8]);

impl<'a> ClockRanges<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let header = data.get(..2).ok_or(Error::Clock)?;
        let count = ClockRangeCount::parse(header)?;
        if data.len() != usize::from(count.bytes()) {
            return Err(Error::Clock);
        }
        let entries = data.get(2..).ok_or(Error::Clock)?;
        let mut previous = 0;
        for entry in entries.chunks_exact(12) {
            let range = RateRange::parse(entry)?;
            if range.min <= previous {
                return Err(Error::Clock);
            }
            previous = range.max;
        }
        Ok(Self(entries))
    }

    pub fn supports(&self, rate: SampleRate) -> bool {
        self.0
            .chunks_exact(12)
            .any(|entry| RateRange::parse(entry).is_ok_and(|range| range.contains(rate.hz())))
    }
}

fn value(data: &[u8], offset: usize) -> Result<u32, Error> {
    let bytes = data.get(offset..offset + 4).ok_or(Error::Clock)?;
    let bytes = <[u8; 4]>::try_from(bytes).map_err(|_| Error::Clock)?;
    Ok(u32::from_le_bytes(bytes))
}
