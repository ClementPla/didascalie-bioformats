//! Which slices, time points and channels to export, from the command line.

use crate::Res;

/// What to write along Z.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZPick {
    /// These slices, one file each.
    Slices(Vec<u32>),
    /// One file holding the maximum over every slice.
    Max,
}

/// `all`, `mid`, `max`, or a list such as `0,3,5-7`.
pub fn parse_z(spec: &str, size: u32) -> Res<ZPick> {
    Ok(match spec {
        "all" => ZPick::Slices((0..size).collect()),
        "mid" => ZPick::Slices(vec![size / 2]),
        "max" => ZPick::Max,
        list => ZPick::Slices(parse_list(list, size, "z")?),
    })
}

/// `all`, `step:N` (every Nth from the first), or a list.
pub fn parse_t(spec: &str, size: u32) -> Res<Vec<u32>> {
    if spec == "all" {
        return Ok((0..size).collect());
    }
    if let Some(n) = spec.strip_prefix("step:") {
        let n: u32 = n.parse().map_err(|_| format!("bad t step: {spec}"))?;
        if n == 0 {
            return Err("t step must be at least 1".into());
        }
        return Ok((0..size).step_by(n as usize).collect());
    }
    parse_list(spec, size, "t")
}

/// `all`, or a list of channels.
pub fn parse_c(spec: &str, size: u32) -> Res<Vec<u32>> {
    if spec == "all" {
        return Ok((0..size).collect());
    }
    parse_list(spec, size, "c")
}

/// Comma-separated indices and inclusive ranges, each below `size`.
fn parse_list(spec: &str, size: u32, axis: &str) -> Res<Vec<u32>> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let bad = || format!("bad {axis} selection: {part}");
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.parse::<u32>().map_err(|_| bad())?, b.parse::<u32>().map_err(|_| bad())?),
            None => {
                let v = part.parse::<u32>().map_err(|_| bad())?;
                (v, v)
            }
        };
        if a > b || b >= size {
            return Err(format!("{axis} {part} is outside 0..{}", size.saturating_sub(1)).into());
        }
        out.extend(a..=b);
    }
    if out.is_empty() {
        return Err(format!("empty {axis} selection").into());
    }
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_specs() {
        assert_eq!(parse_z("all", 3).unwrap(), ZPick::Slices(vec![0, 1, 2]));
        assert_eq!(parse_z("mid", 21).unwrap(), ZPick::Slices(vec![10]));
        assert_eq!(parse_z("mid", 1).unwrap(), ZPick::Slices(vec![0]));
        assert_eq!(parse_z("max", 5).unwrap(), ZPick::Max);
        assert_eq!(parse_z("0,3-4", 5).unwrap(), ZPick::Slices(vec![0, 3, 4]));
        assert!(parse_z("5", 5).is_err());
    }

    #[test]
    fn t_and_c_specs() {
        assert_eq!(parse_t("step:3", 7).unwrap(), vec![0, 3, 6]);
        assert!(parse_t("step:0", 7).is_err());
        assert_eq!(parse_t("1, 2", 7).unwrap(), vec![1, 2]);
        assert_eq!(parse_c("all", 2).unwrap(), vec![0, 1]);
        assert!(parse_c("x", 2).is_err());
        assert!(parse_c("", 2).is_err());
        assert!(parse_c("2-1", 3).is_err());
    }
}
