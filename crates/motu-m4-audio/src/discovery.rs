use crate::AudioError;
use motu_m4_core::Hardware;
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct M4Card {
    index: u32,
    id: String,
    usb_product: u16,
}

impl M4Card {
    pub fn index(&self) -> u32 {
        self.index
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn usb_product(&self) -> u16 {
        self.usb_product
    }
    pub fn pcm_name(&self) -> String {
        format!("hw:CARD={},DEV=0", self.id)
    }
}

pub fn discover_cards(asound: &Path, sound: &Path) -> Result<Vec<M4Card>, AudioError> {
    let entries = match fs::read_dir(asound) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut cards = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(index) = name
            .to_str()
            .and_then(|name| name.strip_prefix("card"))
            .and_then(|index| index.parse::<u32>().ok())
        else {
            continue;
        };
        let usb = match fs::read_to_string(entry.path().join("usbid")) {
            Ok(usb) => usb,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let Some((vendor, product)) = usb.trim().split_once(':') else {
            continue;
        };
        let (Ok(vendor), Ok(product)) = (
            u16::from_str_radix(vendor, 16),
            u16::from_str_radix(product, 16),
        ) else {
            continue;
        };
        if vendor != 0x07fd || !matches!(product, 0x0008 | 0x000b) {
            continue;
        }
        let product_name =
            fs::read_to_string(sound.join(format!("card{index}/device/../product")))?;
        if Hardware::identify(vendor, product, product_name.trim()).is_err() {
            continue;
        }
        let id = fs::read_to_string(entry.path().join("id"))?;
        let id = id.trim();
        if id.is_empty()
            || id.len() > 32
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(AudioError::Invalid(
                "M4 card ID contains unsupported characters",
            ));
        }
        cards.push(M4Card {
            index,
            id: id.to_owned(),
            usb_product: product,
        });
    }
    cards.sort_by_key(M4Card::index);
    Ok(cards)
}

pub fn select_card<'a>(cards: &'a [M4Card], id: Option<&str>) -> Result<&'a M4Card, AudioError> {
    if let Some(id) = id {
        return cards
            .iter()
            .find(|card| card.id() == id)
            .ok_or(AudioError::Invalid(
                "selected M4 card wasn't found; run audio devices",
            ));
    }
    match cards {
        [card] => Ok(card),
        [] => Err(AudioError::Invalid(
            "no MOTU M4 was found; check the USB connection and snd-usb-audio driver",
        )),
        _ => Err(AudioError::Invalid(
            "multiple M4 cards were found; select one with --card",
        )),
    }
}
