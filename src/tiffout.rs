//! Multi-page TIFF output: one greyscale page per channel, all the same size
//! and sample type. Didascalie reads such a file as one multi-channel frame
//! (`pixels::raw::decode` in the app).

use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;

use tiff::encoder::{colortype, Compression, DeflateLevel, Predictor, TiffEncoder};

use crate::planes::Page;
use crate::Res;

/// Write `pages` (width × height each) to `path`; returns the file size.
pub fn write_pages(path: &Path, width: u32, height: u32, pages: &[Page]) -> Res<u64> {
    let mut out = BufWriter::new(File::create(path)?);
    encode(&mut out, width, height, pages)?;
    out.flush()?;
    drop(out);
    Ok(std::fs::metadata(path)?.len())
}

/// Deflate everywhere; the horizontal predictor for integers only, since the
/// encoder refuses it on floats.
pub fn encode<W: Write + Seek>(out: W, width: u32, height: u32, pages: &[Page]) -> Res<()> {
    let float = matches!(pages.first(), Some(Page::F32(_)));
    let mut enc = TiffEncoder::new(out)?
        .with_compression(Compression::Deflate(DeflateLevel::Balanced))
        .with_predictor(if float { Predictor::None } else { Predictor::Horizontal });
    for page in pages {
        match page {
            Page::U8(d) => enc.write_image::<colortype::Gray8>(width, height, d)?,
            Page::U16(d) => enc.write_image::<colortype::Gray16>(width, height, d)?,
            Page::F32(d) => enc.write_image::<colortype::Gray32Float>(width, height, d)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tiff::decoder::{Decoder, DecodingResult};

    use super::*;

    /// What the app's `decode_multipage_tiff` accepts: every page greyscale,
    /// one size, one sample type. Returns the pages as decoded.
    fn decode_like_the_app(bytes: &[u8]) -> (u32, u32, Vec<DecodingResult>) {
        let mut dec = Decoder::new(Cursor::new(bytes)).unwrap();
        let mut pages = Vec::new();
        let mut size = None;
        loop {
            let dims = dec.dimensions().unwrap();
            assert!(matches!(dec.colortype().unwrap(), tiff::ColorType::Gray(_)));
            assert_eq!(*size.get_or_insert(dims), dims);
            pages.push(dec.read_image().unwrap());
            if !dec.more_images() {
                break;
            }
            dec.next_image().unwrap();
        }
        (size.unwrap().0, size.unwrap().1, pages)
    }

    fn round_trip(pages: Vec<Page>) -> Vec<DecodingResult> {
        let mut buf = Cursor::new(Vec::new());
        encode(&mut buf, 3, 2, &pages).unwrap();
        let (w, h, decoded) = decode_like_the_app(buf.get_ref());
        assert_eq!((w, h), (3, 2));
        assert_eq!(decoded.len(), pages.len());
        decoded
    }

    #[test]
    fn u16_pages_read_back_as_channels() {
        let a: Vec<u16> = vec![0, 1, 4095, 65535, 7, 300];
        let b: Vec<u16> = vec![9, 8, 7, 6, 5, 4];
        let got = round_trip(vec![Page::U16(a.clone()), Page::U16(b.clone()), Page::U16(a.clone())]);
        assert!(matches!(&got[0], DecodingResult::U16(v) if *v == a));
        assert!(matches!(&got[1], DecodingResult::U16(v) if *v == b));
    }

    #[test]
    fn u8_and_f32_pages_read_back() {
        let got = round_trip(vec![Page::U8(vec![0, 128, 255, 1, 2, 3]), Page::U8(vec![5; 6])]);
        assert!(matches!(&got[0], DecodingResult::U8(v) if *v == [0, 128, 255, 1, 2, 3]));

        let f = vec![-300.0f32, 0.0, 1.5, 1e6, -0.25, 42.0];
        let got = round_trip(vec![Page::F32(f.clone()), Page::F32(f.clone())]);
        assert!(matches!(&got[1], DecodingResult::F32(v) if *v == f));
    }
}
