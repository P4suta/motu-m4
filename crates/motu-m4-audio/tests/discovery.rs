use motu_m4_audio::{discover_cards, select_card};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "motu-native-discovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn card(&self, index: u32, id: &str, product: &str, usb: &str) {
        let proc = self.0.join(format!("asound/card{index}"));
        let sys = self.0.join(format!("sound/card{index}"));
        fs::create_dir_all(&proc).unwrap();
        fs::create_dir_all(sys.join("device")).unwrap();
        fs::write(proc.join("id"), id).unwrap();
        fs::write(proc.join("usbid"), usb).unwrap();
        fs::write(sys.join("product"), product).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn discovery_uses_product_identity_and_keeps_stable_card_ids() {
    let scratch = Scratch::new();
    scratch.card(9, "M4_1\n", "M4\n", "07fd:0008\n");
    scratch.card(1, "M4\n", "M4\n", "07fd:000b\n");
    scratch.card(2, "M2\n", "M2\n", "07fd:000b\n");
    scratch.card(3, "Other\n", "M4\n", "1234:000b\n");
    let cards = discover_cards(&scratch.0.join("asound"), &scratch.0.join("sound")).unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].index(), 1);
    assert_eq!(cards[0].pcm_name(), "hw:CARD=M4,DEV=0");
    assert_eq!(cards[1].id(), "M4_1");
    assert!(select_card(&cards, None).is_err());
    assert_eq!(select_card(&cards, Some("M4_1")).unwrap().index(), 9);
    assert!(select_card(&cards, Some("M2")).is_err());
    fs::write(scratch.0.join("asound/card1/id"), "M4;type=other").unwrap();
    assert!(discover_cards(&scratch.0.join("asound"), &scratch.0.join("sound")).is_err());
}
