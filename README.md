# didascalie-bioformats

A small command-line program that reads microscopy files with
[bioformats-rs](https://github.com/henriksson-lab/bioformats-rs) (CZI, LIF,
ND2, OME-TIFF and the other formats it knows). It writes the planes
[Didascalie](https://github.com/ClementPla/Didascalie) imports.

It is licensed under the GPL (version 2 or later), because it links
bioformats-rs. Didascalie runs it as a separate process and talks to it via
its command line and JSON output, so Didascalie itself is not bound by the GPL.

## Commands

```
didascalie-bioformats inspect <file>
```

Prints one JSON document describing the file (`protocol` is the version of
this output, which the application checks):

- every series, with its size, Z/C/T counts and pixel type;
- the resolution levels of each series;
- the OME metadata, when present:
  - channel names, colours, excitation and emission wavelengths;
  - physical pixel size in µm, time increment and acquisition date.

```
didascalie-bioformats export <file> --out <dir> [--series S] [--level L]
    [--z all|mid|max|LIST] [--t all|step:N|LIST] [--c all|LIST]
```

Writes one multi-page TIFF per (t, z), named `s{S}_t{t}_z{z}.tif`, with one
greyscale page per channel. `--z max` writes the maximum over Z instead, named
`z max`. `LIST` looks like `0,3,5-7`.

For each file written it prints a JSON line (`"event": "file"`), then a
`"done"` line. Errors go to stderr with exit code 1.

| Source pixels | Stored as |
|---|---|
| uint8, bit | uint8 |
| int8 | uint8, shifted by 128 |
| uint16 | uint16 |
| int16, int32, uint32, float, double | float32 |

An RGB plane becomes three pages: red, green and blue.

Pages are Deflate-compressed, with the horizontal predictor for integer
pages. A plane over 1 GiB per channel is refused; pick a lower `--level`.

## Building

```
cargo build --release
```

`Cargo.lock` pins `zip` to 2.5.0: bioformats-rs at the pinned revision does
not build with zip 2.6.

## Releases

Pushing a version tag (`git tag v0.1.0 && git push origin v0.1.0`) builds
the program for Windows, macOS (Apple silicon and Intel) and Linux, and
attaches the binaries and the licence to a GitHub release. Didascalie's
release builds download a pinned version and ship it next to the
application.
