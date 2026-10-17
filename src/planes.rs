//! Plane geometry and sample conversion: which plane holds (z, c, t), and how
//! the reader's bytes become the samples Didascalie stores.

use bioformats::{DimensionOrder, ImageMetadata, ImageReader, PixelType};

use crate::Res;

/// Samples per pixel in one plane: 3 for RGB planes, else 1.
pub fn rgb_samples(m: &ImageMetadata) -> u32 {
    let total = m.size_c as u64 * m.size_z as u64 * m.size_t as u64;
    if m.image_count == 0 {
        return 1;
    }
    ((total / m.image_count as u64) as u32).max(1)
}

/// Channels as planes count them: `size_c` over the samples of an RGB plane.
pub fn effective_c(m: &ImageMetadata) -> u32 {
    (m.size_c / rgb_samples(m)).max(1)
}

/// Plane number of (z, c, t) for the series' dimension order
/// (Bio-Formats `FormatTools.getIndex`).
pub fn plane_index(m: &ImageMetadata, z: u32, c: u32, t: u32) -> u32 {
    let (sz, sc, st) = (m.size_z, effective_c(m), m.size_t);
    let order: [(u32, u32); 3] = match m.dimension_order {
        DimensionOrder::XYZCT => [(z, sz), (c, sc), (t, st)],
        DimensionOrder::XYZTC => [(z, sz), (t, st), (c, sc)],
        DimensionOrder::XYCZT => [(c, sc), (z, sz), (t, st)],
        DimensionOrder::XYCTZ => [(c, sc), (t, st), (z, sz)],
        DimensionOrder::XYTCZ => [(t, st), (c, sc), (z, sz)],
        DimensionOrder::XYTZC => [(t, st), (z, sz), (c, sc)],
    };
    order[0].0 + order[0].1 * (order[1].0 + order[1].1 * order[2].0)
}

/// Sample type of the written pages: what the app's raw path holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    U8,
    U16,
    F32,
}

impl Sample {
    /// Lossless for every type but `Uint32` above 2^24, which is rare: signed
    /// 8-bit is shifted by 128, wider or signed integers become float.
    pub fn for_pixel_type(px: PixelType) -> Sample {
        match px {
            PixelType::Uint8 | PixelType::Int8 | PixelType::Bit => Sample::U8,
            PixelType::Uint16 => Sample::U16,
            PixelType::Int16
            | PixelType::Int32
            | PixelType::Uint32
            | PixelType::Float32
            | PixelType::Float64 => Sample::F32,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Sample::U8 => "u8",
            Sample::U16 => "u16",
            Sample::F32 => "f32",
        }
    }
}

/// One sample, from the reader's little- or big-endian bytes.
pub fn read_sample(px: PixelType, le: bool, b: &[u8]) -> f64 {
    macro_rules! num {
        ($t:ty, $n:expr) => {{
            let mut a = [0u8; $n];
            a.copy_from_slice(&b[..$n]);
            (if le { <$t>::from_le_bytes(a) } else { <$t>::from_be_bytes(a) }) as f64
        }};
    }
    match px {
        PixelType::Uint8 | PixelType::Bit => b[0] as f64,
        PixelType::Int8 => b[0] as i8 as f64,
        PixelType::Uint16 => num!(u16, 2),
        PixelType::Int16 => num!(i16, 2),
        PixelType::Uint32 => num!(u32, 4),
        PixelType::Int32 => num!(i32, 4),
        PixelType::Float32 => num!(f32, 4),
        PixelType::Float64 => num!(f64, 8),
    }
}

/// One page of samples: a single channel of a plane.
#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl Page {
    pub fn zeros(sample: Sample, len: usize) -> Page {
        match sample {
            Sample::U8 => Page::U8(vec![0; len]),
            Sample::U16 => Page::U16(vec![0; len]),
            Sample::F32 => Page::F32(vec![0.0; len]),
        }
    }

    /// Store a sample read as `px` at `i`.
    #[inline]
    pub fn set(&mut self, i: usize, px: PixelType, v: f64) {
        match self {
            Page::U8(d) => d[i] = if px == PixelType::Int8 { (v + 128.0) as u8 } else { v as u8 },
            Page::U16(d) => d[i] = v as u16,
            Page::F32(d) => d[i] = v as f32,
        }
    }

    /// Keep the larger value at every pixel (maximum projection).
    pub fn max_with(&mut self, other: &Page) {
        match (self, other) {
            (Page::U8(a), Page::U8(b)) => a.iter_mut().zip(b).for_each(|(x, y)| *x = (*x).max(*y)),
            (Page::U16(a), Page::U16(b)) => a.iter_mut().zip(b).for_each(|(x, y)| *x = (*x).max(*y)),
            (Page::F32(a), Page::F32(b)) => a.iter_mut().zip(b).for_each(|(x, y)| *x = x.max(*y)),
            _ => unreachable!("pages of one series share a sample type"),
        }
    }
}

/// Largest plane, per channel, that is read whole: past it, pick a lower
/// resolution level.
pub const MAX_PAGE_BYTES: u64 = 1 << 30;
/// Bytes read from the file per call, so a large plane is never held twice.
const STRIP_BYTES: u64 = 32 << 20;

/// Read plane `index` of the current series and level as one page per sample
/// (one for a greyscale plane, three for RGB).
pub fn read_plane(r: &mut ImageReader, m: &ImageMetadata, index: u32) -> Res<Vec<Page>> {
    let spp = rgb_samples(m) as usize;
    let bps = m.pixel_type.bytes_per_sample();
    let sample = Sample::for_pixel_type(m.pixel_type);
    let (w, h) = (m.size_x as usize, m.size_y as usize);
    let page_bytes = (w * h) as u64 * sample_bytes(sample);
    if page_bytes > MAX_PAGE_BYTES {
        return Err(format!(
            "a {w}×{h} plane is {} MB per channel; pick a lower resolution level",
            page_bytes >> 20
        )
        .into());
    }
    let mut pages: Vec<Page> = (0..spp).map(|_| Page::zeros(sample, w * h)).collect();

    let row_bytes = (w * spp * bps) as u64;
    let strip_h = (STRIP_BYTES / row_bytes.max(1)).clamp(1, h as u64) as usize;
    let mut y0 = 0;
    while y0 < h {
        let sh = strip_h.min(h - y0);
        let bytes = r.open_bytes_region(index, 0, y0 as u32, w as u32, sh as u32)?;
        let need = w * sh * spp * bps;
        if bytes.len() < need {
            return Err(format!("short read: {} of {} bytes", bytes.len(), need).into());
        }
        for yy in 0..sh {
            for x in 0..w {
                let out = (y0 + yy) * w + x;
                for (k, page) in pages.iter_mut().enumerate() {
                    let off = if m.is_interleaved || spp == 1 {
                        ((yy * w + x) * spp + k) * bps
                    } else {
                        // Planar RGB: one whole sample plane after another.
                        ((k * sh + yy) * w + x) * bps
                    };
                    page.set(out, m.pixel_type, read_sample(m.pixel_type, m.is_little_endian, &bytes[off..off + bps]));
                }
            }
        }
        y0 += sh;
    }
    Ok(pages)
}

fn sample_bytes(s: Sample) -> u64 {
    match s {
        Sample::U8 => 1,
        Sample::U16 => 2,
        Sample::F32 => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(order: DimensionOrder, z: u32, c: u32, t: u32) -> ImageMetadata {
        ImageMetadata {
            size_z: z,
            size_c: c,
            size_t: t,
            image_count: z * c * t,
            dimension_order: order,
            ..Default::default()
        }
    }

    #[test]
    fn plane_index_follows_the_dimension_order() {
        let m = meta(DimensionOrder::XYCZT, 4, 3, 2);
        assert_eq!(plane_index(&m, 0, 0, 0), 0);
        assert_eq!(plane_index(&m, 0, 2, 0), 2);
        assert_eq!(plane_index(&m, 1, 0, 0), 3);
        assert_eq!(plane_index(&m, 3, 2, 1), 2 + 3 * (3 + 4));

        let m = meta(DimensionOrder::XYZTC, 4, 3, 2);
        assert_eq!(plane_index(&m, 1, 0, 0), 1);
        assert_eq!(plane_index(&m, 0, 0, 1), 4);
        assert_eq!(plane_index(&m, 0, 1, 0), 8);
    }

    #[test]
    fn rgb_planes_count_one_channel() {
        // An RGB series: size_c counts samples, planes hold all three.
        let mut m = meta(DimensionOrder::XYCZT, 2, 3, 1);
        m.image_count = 2;
        assert_eq!(rgb_samples(&m), 3);
        assert_eq!(effective_c(&m), 1);
        assert_eq!(plane_index(&m, 1, 0, 0), 1);
    }

    #[test]
    fn samples_convert_without_loss() {
        assert_eq!(Sample::for_pixel_type(PixelType::Int8), Sample::U8);
        assert_eq!(Sample::for_pixel_type(PixelType::Uint16), Sample::U16);
        assert_eq!(Sample::for_pixel_type(PixelType::Int16), Sample::F32);
        assert_eq!(Sample::for_pixel_type(PixelType::Float64), Sample::F32);

        let mut p = Page::zeros(Sample::U8, 2);
        p.set(0, PixelType::Int8, read_sample(PixelType::Int8, true, &[0x80])); // -128
        p.set(1, PixelType::Int8, read_sample(PixelType::Int8, true, &[0x7f])); // 127
        assert_eq!(p, Page::U8(vec![0, 255]));

        assert_eq!(read_sample(PixelType::Uint16, true, &[0x34, 0x12]), 0x1234 as f64);
        assert_eq!(read_sample(PixelType::Uint16, false, &[0x12, 0x34]), 0x1234 as f64);
        assert_eq!(read_sample(PixelType::Int16, true, &(-300i16).to_le_bytes()), -300.0);
        assert_eq!(read_sample(PixelType::Uint32, false, &70000u32.to_be_bytes()), 70000.0);
        assert_eq!(read_sample(PixelType::Float64, true, &2.5f64.to_le_bytes()), 2.5);

        let mut f = Page::zeros(Sample::F32, 1);
        f.set(0, PixelType::Int16, -300.0);
        assert_eq!(f, Page::F32(vec![-300.0]));
    }

    #[test]
    fn max_projection_keeps_the_brightest() {
        let mut a = Page::U16(vec![1, 9, 3]);
        a.max_with(&Page::U16(vec![5, 2, 3]));
        assert_eq!(a, Page::U16(vec![5, 9, 3]));
        let mut f = Page::F32(vec![-1.0, 0.5]);
        f.max_with(&Page::F32(vec![-2.0, 1.5]));
        assert_eq!(f, Page::F32(vec![-1.0, 1.5]));
    }
}
