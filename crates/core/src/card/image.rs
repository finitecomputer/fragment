//! What a shot or an upload must be: a JPEG of the card's size, within its bytes.

use super::limits::{BYTES_MAX, HEIGHT, WIDTH};
use serde::{Deserialize, Serialize};

/// Why an image (a shot's, or an upload's) is no card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFault {
    Empty,
    TooLarge { size: usize },
    NotJpeg,
    Size { width: u16, height: u16 },
    /// An upload whose JPEG is cut short or has a part missing
    /// (`media::jpeg_whole`).
    NotWhole,
}

impl ImageFault {
    pub fn message(self) -> String {
        match self {
            ImageFault::Empty => "the image is empty".into(),
            ImageFault::TooLarge { size } => format!("the image is {size} bytes, over the {BYTES_MAX} a card holds"),
            ImageFault::NotJpeg => "the image is no JPEG: a card is one".into(),
            ImageFault::Size { width, height } => format!("the image is {width}×{height}, not the card's {WIDTH}×{HEIGHT}"),
            ImageFault::NotWhole => "the image is no whole JPEG: it is cut short, or a part of it is missing".into(),
        }
    }
}

/// Whether an upload's `bytes` are a card: a card's image (`check_image`)
/// that is a whole JPEG too (`media::jpeg_whole`), so one cut short never
/// replaces the card. A shot is Chrome's own encoding, held to
/// `check_image` alone; an upload is anyone's bytes.
pub fn check_upload(bytes: &[u8]) -> Result<(), ImageFault> {
    check_image(bytes)?;
    if crate::media::jpeg_whole(bytes) {
        Ok(())
    } else {
        Err(ImageFault::NotWhole)
    }
}

/// Whether `bytes` are a card: a JPEG of the viewport's size, within the
/// cap. An upload is held to it too, so every card is one shape, which the
/// shell lays out without asking.
pub fn check_image(bytes: &[u8]) -> Result<(), ImageFault> {
    if bytes.is_empty() {
        return Err(ImageFault::Empty);
    }
    if bytes.len() > BYTES_MAX {
        return Err(ImageFault::TooLarge { size: bytes.len() });
    }
    match crate::media::jpeg_size(bytes) {
        None => Err(ImageFault::NotJpeg),
        Some((WIDTH, HEIGHT)) => Ok(()),
        Some((width, height)) => Err(ImageFault::Size { width, height }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG header of `w` × `h`: SOI, then SOF0.
    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend([0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xD9]);
        b
    }

    /// Goal: a card is a JPEG of the viewport, within the cap; anything
    /// else is refused, saying what it is.
    #[test]
    fn a_card_is_a_jpeg_of_the_viewport() {
        assert_eq!(check_image(&jpeg(WIDTH, HEIGHT)), Ok(()));
        assert_eq!(check_image(&[]), Err(ImageFault::Empty));
        assert_eq!(check_image(b"\x89PNG\r\n\x1a\n"), Err(ImageFault::NotJpeg));
        assert_eq!(check_image(&jpeg(2560, 1600)), Err(ImageFault::Size { width: 2560, height: 1600 }));
        let mut big = jpeg(WIDTH, HEIGHT);
        big.resize(BYTES_MAX + 1, 0);
        assert_eq!(check_image(&big), Err(ImageFault::TooLarge { size: BYTES_MAX + 1 }));
        big.truncate(BYTES_MAX);
        assert_eq!(check_image(&big), Ok(()), "exactly the cap is a card");
    }

    /// Goal: an upload is a card's image that is a whole JPEG too: one cut
    /// short (Alex's frame header that ends early, a header and no image)
    /// is refused, as is anything `check_image` refuses, first.
    #[test]
    fn an_upload_is_a_whole_jpeg() {
        let truncated = [0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x03, 0x20, 0x05, 0x00];
        assert_eq!(check_image(&truncated), Ok(()), "which its size alone takes");
        assert_eq!(check_upload(&truncated), Err(ImageFault::NotWhole));
        assert_eq!(check_upload(&jpeg(WIDTH, HEIGHT)), Err(ImageFault::NotWhole), "a header and its end");
        assert_eq!(check_upload(&jpeg(640, 400)), Err(ImageFault::Size { width: 640, height: 400 }), "the size said first");
        assert_eq!(check_upload(&[]), Err(ImageFault::Empty));
        // a whole one of the card's size
        let mut whole = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00];
        whole.extend([1u8; 64]);
        whole.extend(&jpeg(WIDTH, HEIGHT)[2..21]);
        whole.extend([0xFF, 0xC4, 0x00, 0x13, 0x00]);
        whole.extend([0u8; 16]);
        whole.extend([0xFF, 0xDA, 0x00, 0x0C, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0x00, 0x3F, 0x00, 0x12, 0x34, 0xFF, 0xD9]);
        assert_eq!(check_upload(&whole), Ok(()));
        assert!(ImageFault::NotWhole.message().contains("no whole JPEG"));
    }
}
