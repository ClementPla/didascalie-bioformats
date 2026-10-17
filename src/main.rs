//! didascalie-bioformats: reads microscopy files (CZI, LIF, ND2, OME-TIFF and
//! the other formats bioformats-rs knows) for Didascalie's import.
//!
//!   didascalie-bioformats inspect <file>
//!       One JSON document: the series, their sizes, levels and channels.
//!
//!   didascalie-bioformats export <file> --out <dir> [--series S] [--level L]
//!                                [--z all|mid|max|LIST] [--t all|step:N|LIST]
//!                                [--c all|LIST]
//!       One multi-page TIFF per (t, z), one page per channel, named
//!       `s{S}_t{t}_z{z}.tif` (`z` is `max` for a projection). One JSON line
//!       per file written, then a `done` line. LIST is like `0,3,5-7`.
//!
//! It is GPL (it links bioformats-rs) and runs as its own process, so the
//! application that starts it is not bound by that licence.

mod planes;
mod select;
mod tiffout;

use std::path::{Path, PathBuf};
use std::time::Instant;

use bioformats::ImageReader;
use serde_json::{json, Value};

use planes::{effective_c, plane_index, read_plane, rgb_samples, Page, Sample};
use select::{parse_c, parse_t, parse_z, ZPick};

pub type Res<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Version of the JSON this program prints, checked by the application.
/// Bump it on any change a caller would notice.
const PROTOCOL: u32 = 1;

const USAGE: &str = "usage: didascalie-bioformats inspect <file>\n       \
    didascalie-bioformats export <file> --out <dir> [--series S] [--level L] \
    [--z all|mid|max|LIST] [--t all|step:N|LIST] [--c all|LIST]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("inspect") if args.len() == 2 => inspect(Path::new(&args[1])),
        Some("export") if args.len() >= 2 => ExportArgs::parse(&args[1..]).and_then(|a| export(&a)),
        Some("--version") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(USAGE.into()),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Resolution levels stay inside their series rather than becoming series of
/// their own, so a series is one image.
fn open(path: &Path) -> Res<ImageReader> {
    Ok(ImageReader::open_with_flattened_resolutions(path, false)?)
}

// ── inspect ──────────────────────────────────────────────────────────────────

fn inspect(path: &Path) -> Res<()> {
    let t0 = Instant::now();
    let mut r = open(path)?;
    let open_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let ome = r.ome_metadata();
    let mut series = Vec::new();
    for s in 0..r.series_count() {
        let image = ome.as_ref().and_then(|o| o.images.get(s));
        series.push(series_json(&mut r, s, image).unwrap_or_else(|e| json!({ "index": s, "error": e.to_string() })));
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "protocol": PROTOCOL,
            "file": path.display().to_string(),
            "openMs": open_ms,
            "hasOmeMetadata": ome.is_some(),
            "series": series,
        }))?
    );
    Ok(())
}

fn series_json(r: &mut ImageReader, s: usize, image: Option<&bioformats::OmeImage>) -> Res<Value> {
    r.set_series(s)?;
    let m = r.metadata().clone();
    let mut levels = Vec::new();
    for level in 0..r.resolution_count() {
        r.set_resolution(level)?;
        levels.push(json!({ "width": r.metadata().size_x, "height": r.metadata().size_y }));
    }
    r.set_resolution(0)?;

    let (c, spp) = (effective_c(&m), rgb_samples(&m));
    let ome_channels = image.map(|i| i.channels.as_slice()).unwrap_or(&[]);
    let mut channels = Vec::new();
    for ci in 0..c {
        let oc = ome_channels.get(ci as usize);
        let name = oc.and_then(|o| non_empty(&o.name));
        for k in 0..spp {
            // An RGB plane is written as three pages, shown red, green, blue.
            let (name, color) = if spp == 3 {
                let rgb = ["R", "G", "B"][k as usize];
                let color = ["#ff0000", "#00ff00", "#0000ff"][k as usize];
                (Some(name.as_deref().map_or(rgb.to_string(), |n| format!("{n} {rgb}"))), Some(color.to_string()))
            } else {
                (name.clone(), oc.and_then(|o| o.color).map(ome_color))
            };
            channels.push(json!({
                "source": ci,
                "sample": k,
                "name": name,
                "color": color,
                "excitationNm": oc.and_then(|o| o.excitation_wavelength),
                "emissionNm": oc.and_then(|o| o.emission_wavelength),
                "fluor": oc.and_then(|o| non_empty(&o.fluor)),
                "contrastMethod": oc.and_then(|o| non_empty(&o.contrast_method)),
            }));
        }
    }

    Ok(json!({
        "index": s,
        "name": image.and_then(|i| non_empty(&i.name)),
        "width": m.size_x,
        "height": m.size_y,
        "sizeZ": m.size_z,
        "sizeC": c,
        "sizeT": m.size_t,
        "samplesPerPixel": spp,
        "pixelType": format!("{:?}", m.pixel_type),
        "bitsPerPixel": m.bits_per_pixel,
        "sample": Sample::for_pixel_type(m.pixel_type).name(),
        "dimensionOrder": format!("{:?}", m.dimension_order),
        "indexed": m.is_indexed,
        "thumbnail": m.thumbnail,
        "levels": levels,
        "physicalSizeUm": image.map(|i| json!({
            "x": i.physical_size_x, "y": i.physical_size_y, "z": i.physical_size_z,
        })),
        "timeIncrementS": image.and_then(|i| i.time_increment),
        "acquisitionDate": image.and_then(|i| non_empty(&i.acquisition_date)),
        "channels": channels,
    }))
}

/// Some readers (LIF) give blank names: those are no name.
fn non_empty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// OME packs a colour as signed RGBA.
fn ome_color(packed: i32) -> String {
    let v = packed as u32;
    format!("#{:02x}{:02x}{:02x}", v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff)
}

// ── export ───────────────────────────────────────────────────────────────────

struct ExportArgs {
    file: PathBuf,
    out: PathBuf,
    series: usize,
    level: usize,
    z: String,
    t: String,
    c: String,
}

impl ExportArgs {
    fn parse(args: &[String]) -> Res<ExportArgs> {
        let mut a = ExportArgs {
            file: PathBuf::from(&args[0]),
            out: PathBuf::new(),
            series: 0,
            level: 0,
            z: "all".into(),
            t: "all".into(),
            c: "all".into(),
        };
        let mut it = args[1..].iter();
        while let Some(flag) = it.next() {
            let value = it.next().ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
            match flag.as_str() {
                "--out" => a.out = PathBuf::from(value),
                "--series" => a.series = value.parse().map_err(|_| format!("bad series: {value}"))?,
                "--level" => a.level = value.parse().map_err(|_| format!("bad level: {value}"))?,
                "--z" => a.z = value.clone(),
                "--t" => a.t = value.clone(),
                "--c" => a.c = value.clone(),
                _ => return Err(format!("unknown option {flag}\n{USAGE}").into()),
            }
        }
        if a.out.as_os_str().is_empty() {
            return Err(format!("--out is required\n{USAGE}").into());
        }
        Ok(a)
    }
}

fn export(a: &ExportArgs) -> Res<()> {
    let t0 = Instant::now();
    let mut r = open(&a.file)?;
    if a.series >= r.series_count() {
        return Err(format!("series {} does not exist ({} series)", a.series, r.series_count()).into());
    }
    r.set_series(a.series)?;
    if a.level >= r.resolution_count() {
        return Err(format!("level {} does not exist ({} levels)", a.level, r.resolution_count()).into());
    }
    r.set_resolution(a.level)?;
    let m = r.metadata().clone();
    let zs = parse_z(&a.z, m.size_z)?;
    let ts = parse_t(&a.t, m.size_t)?;
    let cs = parse_c(&a.c, effective_c(&m))?;
    std::fs::create_dir_all(&a.out)?;

    let (mut files, mut total) = (0u64, 0u64);
    for &t in &ts {
        let picks: Vec<Option<u32>> = match &zs {
            ZPick::Slices(z) => z.iter().map(|&z| Some(z)).collect(),
            ZPick::Max => vec![None],
        };
        for z in picks {
            let mut pages: Vec<Page> = Vec::new();
            for &c in &cs {
                let channel = match z {
                    Some(z) => read_plane(&mut r, &m, plane_index(&m, z, c, t))?,
                    None => max_over_z(&mut r, &m, c, t)?,
                };
                pages.extend(channel);
            }
            let z_name = z.map_or("max".to_string(), |z| z.to_string());
            let path = a.out.join(format!("s{}_t{}_z{}.tif", a.series, t, z_name));
            let bytes = tiffout::write_pages(&path, m.size_x, m.size_y, &pages)?;
            files += 1;
            total += bytes;
            println!(
                "{}",
                json!({
                    "event": "file",
                    "path": path.display().to_string(),
                    "series": a.series,
                    "t": t,
                    "z": z.map_or(json!("max"), |z| json!(z)),
                    "width": m.size_x,
                    "height": m.size_y,
                    "channels": pages.len(),
                    "sample": Sample::for_pixel_type(m.pixel_type).name(),
                    "bytes": bytes,
                })
            );
        }
    }
    println!(
        "{}",
        json!({ "event": "done", "protocol": PROTOCOL, "files": files, "bytes": total, "ms": t0.elapsed().as_secs_f64() * 1000.0 })
    );
    Ok(())
}

/// Maximum over every slice of channel `c` at time `t`, one page per sample.
fn max_over_z(r: &mut ImageReader, m: &bioformats::ImageMetadata, c: u32, t: u32) -> Res<Vec<Page>> {
    let mut acc = read_plane(r, m, plane_index(m, 0, c, t))?;
    for z in 1..m.size_z {
        let next = read_plane(r, m, plane_index(m, z, c, t))?;
        acc.iter_mut().zip(&next).for_each(|(a, b)| a.max_with(b));
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ome_colours_unpack_as_rgba() {
        assert_eq!(ome_color(0x00ff00ffu32 as i32), "#00ff00");
        assert_eq!(ome_color(-16776961), "#ff0000"); // 0xff0000ff
        assert_eq!(ome_color(-1), "#ffffff");
    }

    #[test]
    fn blank_names_are_none() {
        assert_eq!(non_empty(&Some("  ".into())), None);
        assert_eq!(non_empty(&None), None);
        assert_eq!(non_empty(&Some("TagYFP".into())).as_deref(), Some("TagYFP"));
    }

    #[test]
    fn export_args_need_out() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(ExportArgs::parse(&args(&["f.czi"])).is_err());
        let a = ExportArgs::parse(&args(&["f.czi", "--out", "o", "--z", "max", "--series", "2"])).unwrap();
        assert_eq!((a.series, a.z.as_str(), a.t.as_str()), (2, "max", "all"));
        assert!(ExportArgs::parse(&args(&["f.czi", "--out"])).is_err());
        assert!(ExportArgs::parse(&args(&["f.czi", "--out", "o", "--bogus", "1"])).is_err());
    }
}
