//! Census of building-interior façade donors: the OLD 3×3-tile-window pick against
//! the NEW per-tile pick, per tile. DIAGNOSTIC ONLY — the measurement receipt for
//! the per-tile [`InteriorEstimate`] (2026-08-23): how many enclosed receiver pixels
//! used to take their façade from a NEIGHBOUR tile, how many now have no donor at
//! all, and — given two HM3 roots painted before and after — how the bytes actually
//! moved, split into (A) enclosed pixels whose old donor was cross-tile, (B) the
//! other enclosed pixels, and (C) outdoor pixels, the geometry-only control the
//! donor change cannot touch.
//!
//! The old donor is replayed verbatim from the retired `bake_interior_donors`: the
//! exact EDT over the 1536×1536 lattice of the 3×3 tile window, sites = outdoor
//! pixels of any window tile, queries = enclosed pixels of the centre tile, and a
//! neighbour beyond the world edge = a `Default`-filled (enclosed) raster.
//!
//! Usage: interior_donor_census --h3r4 <dir> --prepared <root> --zoom <z>
//!   --cells <file, one R4 hex per line> [--layers road,rail]
//!   [--old <hm3 root>] [--new <hm3 root>] [--tsv <path>]
//!
//! Runs under the painters' environment (`QM_VECTOR_BUILDINGS`,
//! `QM_OBSTACLES_ALLOW_PARTIAL`); a raster-fallback cell reports every tile as
//! `mode=raster` with zero counts. HM3 tiles live at `<root>/<layer>/<z>/<x>/<y>.bin`;
//! an absent file is a tile of all `NO_DATA`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use h3o::CellIndex;
use noise_compute::envelope::EnvelopeClass;
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use raster_reader::RealRasters;
use rayon::prelude::*;
use tile_painter::region_runner::{read_r4_file, region_tiles};
use tile_painter::source_loader_obstacle::{
    nearest_site_offsets, InteriorEstimate, ObstacleData, NO_DONOR,
};
use tile_painter::wire_hm3::{dequantise_lden, read_tile_bytes, NO_DATA};

const USAGE: &str = "usage: interior_donor_census --h3r4 <dir> --prepared <root> --zoom <z> \
--cells <file> [--layers road,rail] [--old <hm3 root>] [--new <hm3 root>] [--tsv <path>]";

const TILE_CELLS: usize = TILE_PX * TILE_PX;

/// The centre member of the row-major 3×3 tile window (`ordinal = (dy + 1) * 3 + dx + 1`).
const CENTRE_TILE_ORDINAL: usize = 4;

/// What the retired Pass A supplied for a window member beyond the world edge:
/// enclosed everywhere, so never a donor site.
static WORLD_EDGE_CLASS_RASTER: [u8; TILE_CELLS] = [EnvelopeClass::Default as u8; TILE_CELLS];

struct Args {
    h3r4_dir: PathBuf,
    prepared_root: PathBuf,
    zoom: u8,
    cells_file: PathBuf,
    layers: Vec<String>,
    old_root: Option<PathBuf>,
    new_root: Option<PathBuf>,
    tsv_path: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut h3r4_dir = None;
    let mut prepared_root = None;
    let mut zoom = None;
    let mut cells_file = None;
    let mut layers = vec!["road".to_string(), "rail".to_string()];
    let mut old_root = None;
    let mut new_root = None;
    let mut tsv_path = None;
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].as_str();
        if !matches!(
            flag,
            "--h3r4"
                | "--prepared"
                | "--zoom"
                | "--cells"
                | "--layers"
                | "--old"
                | "--new"
                | "--tsv"
        ) {
            return Err(format!("unknown option {flag}\n{USAGE}"));
        }
        let value = argv
            .get(i + 1)
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag {
            "--h3r4" => h3r4_dir = Some(PathBuf::from(value)),
            "--prepared" => prepared_root = Some(PathBuf::from(value)),
            "--zoom" => {
                zoom = Some(
                    value
                        .parse::<u8>()
                        .ok()
                        .filter(|z| *z <= 20)
                        .ok_or_else(|| format!("bad --zoom {value}"))?,
                );
            }
            "--cells" => cells_file = Some(PathBuf::from(value)),
            "--layers" => {
                layers = value
                    .split(',')
                    .map(str::trim)
                    .filter(|layer| !layer.is_empty())
                    .map(str::to_string)
                    .collect();
                if layers.is_empty() {
                    return Err(format!("--layers needs at least one layer\n{USAGE}"));
                }
            }
            "--old" => old_root = Some(PathBuf::from(value)),
            "--new" => new_root = Some(PathBuf::from(value)),
            "--tsv" => tsv_path = Some(PathBuf::from(value)),
            _ => unreachable!("flag list guarded above"),
        }
        i += 2;
    }
    if old_root.is_some() != new_root.is_some() {
        return Err(format!("--old and --new go together\n{USAGE}"));
    }
    Ok(Args {
        h3r4_dir: h3r4_dir.ok_or_else(|| format!("--h3r4 is required\n{USAGE}"))?,
        prepared_root: prepared_root.ok_or_else(|| format!("--prepared is required\n{USAGE}"))?,
        zoom: zoom.ok_or_else(|| format!("--zoom is required\n{USAGE}"))?,
        cells_file: cells_file.ok_or_else(|| format!("--cells is required\n{USAGE}"))?,
        layers,
        old_root,
        new_root,
        tsv_path,
    })
}

/// Window-frame (1536×1536) ordinal of the retired donor → which of the nine
/// window tiles it lies in (row-major, centre = [`CENTRE_TILE_ORDINAL`]).
fn window_tile_ordinal(window_ordinal: u32) -> usize {
    let (x, y) = (
        window_ordinal as usize % (TILE_PX * 3),
        window_ordinal as usize / (TILE_PX * 3),
    );
    (y / TILE_PX) * 3 + (x / TILE_PX)
}

/// The retired `bake_interior_donors`, replayed verbatim: one exact EDT over the
/// 3×3 window's 1536×1536 lattice, queried for the centre tile only. Returns the
/// donor's window-frame ordinal per centre pixel, or [`NO_DONOR`].
fn replay_halo_donors(halo_classes: [&[u8]; 9]) -> Vec<u32> {
    debug_assert!(halo_classes
        .iter()
        .all(|classes| classes.len() == TILE_CELLS));
    nearest_site_offsets(
        TILE_PX * 3,
        TILE_PX..TILE_PX * 2,
        TILE_PX..TILE_PX * 2,
        |x, y| {
            let tile_ordinal = (y / TILE_PX) * 3 + (x / TILE_PX);
            let pixel_ordinal = (y % TILE_PX) * TILE_PX + (x % TILE_PX);
            EnvelopeClass::from_u8(halo_classes[tile_ordinal][pixel_ordinal])
                == EnvelopeClass::Outdoor
        },
        |x, y| {
            let pixel_ordinal = (y - TILE_PX) * TILE_PX + (x - TILE_PX);
            EnvelopeClass::from_u8(halo_classes[CENTRE_TILE_ORDINAL][pixel_ordinal])
                .delta_db()
                .is_some()
        },
    )
}

/// Tile `(x, y)`'s 3×3-window member `ordinal`; `None` beyond the world edge.
fn window_member_xy(x: u32, y: u32, zoom: u8, ordinal: usize) -> Option<(u32, u32)> {
    let limit = 1_i64 << zoom;
    let member_x = i64::from(x) + ordinal as i64 % 3 - 1;
    let member_y = i64::from(y) + ordinal as i64 / 3 - 1;
    ((0..limit).contains(&member_x) && (0..limit).contains(&member_y))
        .then_some((member_x as u32, member_y as u32))
}

/// The 3×3 class window around `(x, y)` out of the cell's bake cache, a world-edge
/// member being the Default-filled raster — exactly what the retired Pass A handed
/// `bake_interior_donors`.
fn halo_class_window(
    bakes: &BTreeMap<(u32, u32), InteriorEstimate>,
    x: u32,
    y: u32,
    zoom: u8,
) -> [&[u8]; 9] {
    std::array::from_fn(|ordinal| {
        window_member_xy(x, y, zoom, ordinal).map_or(&WORLD_EDGE_CLASS_RASTER[..], |member| {
            bakes
                .get(&member)
                .expect("every in-world window member is baked before the replay")
                .classes()
        })
    })
}

/// Per-tile donor geometry of the enclosed pixels, old pick vs new pick. An
/// old in-tile pick always equals the new pick (both transforms take the
/// lexicographic-min site and the centre tile keeps its order in both frames),
/// and an old `NO_DONOR` means no outdoor pixel in the whole window, so these
/// four counters are the complete story: (A) = `old_cross_tile`.
#[derive(Clone, Copy, Default)]
struct DonorGeometryCounts {
    enclosed: u64,
    /// Old donor exists and lies in a neighbour tile — the population the change affects.
    old_cross_tile: u64,
    old_none: u64,
    new_none: u64,
}

impl DonorGeometryCounts {
    fn merge(&mut self, other: &Self) {
        self.enclosed += other.enclosed;
        self.old_cross_tile += other.old_cross_tile;
        self.old_none += other.old_none;
        self.new_none += other.new_none;
    }
}

/// The three pixel populations of the byte drift; (A) is the donor-semantics
/// population, (B) and (C) are controls — (C) cannot be touched by the donor change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Population {
    EnclosedOldCrossTile = 0,
    EnclosedOther = 1,
    Outdoor = 2,
}

const POPULATIONS: [Population; 3] = [
    Population::EnclosedOldCrossTile,
    Population::EnclosedOther,
    Population::Outdoor,
];

impl Population {
    fn tsv_prefix(self) -> &'static str {
        match self {
            Population::EnclosedOldCrossTile => "a",
            Population::EnclosedOther => "b",
            Population::Outdoor => "c",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Population::EnclosedOldCrossTile => "A enclosed, old donor cross-tile",
            Population::EnclosedOther => "B enclosed, other",
            Population::Outdoor => "C outdoor (control)",
        }
    }
}

/// Classify every pixel of one tile and count the donor geometry.
fn count_donor_geometry(
    classes: &[u8],
    old_donors: &[u32],
    new_donors: &[u32],
) -> (DonorGeometryCounts, Vec<Population>) {
    let mut counts = DonorGeometryCounts::default();
    let populations = (0..TILE_CELLS)
        .map(|pixel| {
            if EnvelopeClass::from_u8(classes[pixel]).delta_db().is_none() {
                return Population::Outdoor;
            }
            counts.enclosed += 1;
            counts.old_none += u64::from(old_donors[pixel] == NO_DONOR);
            counts.new_none += u64::from(new_donors[pixel] == NO_DONOR);
            if old_donors[pixel] != NO_DONOR
                && window_tile_ordinal(old_donors[pixel]) != CENTRE_TILE_ORDINAL
            {
                counts.old_cross_tile += 1;
                Population::EnclosedOldCrossTile
            } else {
                Population::EnclosedOther
            }
        })
        .collect();
    (counts, populations)
}

/// Byte drift of one pixel population between the old and the new HM3 tile.
#[derive(Clone, Copy, Default)]
struct ByteDrift {
    pixels: u64,
    differs: u64,
    to_nodata: u64,
    from_nodata: u64,
    finite_both: u64,
    over_0_5_db: u64,
    over_1_db: u64,
    over_2_db: u64,
    over_6_db: u64,
    max_abs_db: f64,
}

impl ByteDrift {
    fn observe(&mut self, old: u8, new: u8) {
        self.pixels += 1;
        if old != new {
            self.differs += 1;
        }
        let old_db = dequantise_lden(old);
        let new_db = dequantise_lden(new);
        match (old_db.is_finite(), new_db.is_finite()) {
            (true, false) => self.to_nodata += 1,
            (false, true) => self.from_nodata += 1,
            (true, true) => {
                self.finite_both += 1;
                let delta = (new_db - old_db).abs();
                self.over_0_5_db += u64::from(delta > 0.5);
                self.over_1_db += u64::from(delta > 1.0);
                self.over_2_db += u64::from(delta > 2.0);
                self.over_6_db += u64::from(delta > 6.0);
                self.max_abs_db = self.max_abs_db.max(delta);
            }
            (false, false) => {}
        }
    }

    fn merge(&mut self, other: &Self) {
        self.pixels += other.pixels;
        self.differs += other.differs;
        self.to_nodata += other.to_nodata;
        self.from_nodata += other.from_nodata;
        self.finite_both += other.finite_both;
        self.over_0_5_db += other.over_0_5_db;
        self.over_1_db += other.over_1_db;
        self.over_2_db += other.over_2_db;
        self.over_6_db += other.over_6_db;
        self.max_abs_db = self.max_abs_db.max(other.max_abs_db);
    }
}

/// One tile's census: the geometry row plus one drift triple per `--layers` entry.
struct TileCensus {
    r4: u64,
    x: u32,
    y: u32,
    vector_mode: bool,
    geometry: DonorGeometryCounts,
    layers: Vec<[ByteDrift; 3]>,
}

impl TileCensus {
    /// A tile of a raster-fallback cell: no classes, no donors, nothing to compare.
    fn raster_fallback(r4: u64, x: u32, y: u32, layer_rows: usize) -> Self {
        TileCensus {
            r4,
            x,
            y,
            vector_mode: false,
            geometry: DonorGeometryCounts::default(),
            layers: vec![[ByteDrift::default(); 3]; layer_rows],
        }
    }
}

/// `<root>/<layer>/<zoom>/<x>/<y>.bin`, decoded; an absent file is all `NO_DATA`.
fn read_hm3_or_nodata(root: &Path, layer: &str, zoom: u8, x: u32, y: u32) -> Result<Vec<u8>> {
    let path = root
        .join(layer)
        .join(zoom.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin"));
    match std::fs::read(&path) {
        Ok(compressed) => {
            let cells = read_tile_bytes(&compressed).with_context(|| path.display().to_string())?;
            if cells.len() != TILE_CELLS {
                bail!(
                    "{}: {} cells, expected {TILE_CELLS}",
                    path.display(),
                    cells.len()
                );
            }
            Ok(cells)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![NO_DATA; TILE_CELLS]),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn census_tile(
    args: &Args,
    bakes: &BTreeMap<(u32, u32), InteriorEstimate>,
    r4: u64,
    x: u32,
    y: u32,
) -> Result<TileCensus> {
    let centre = &bakes[&(x, y)];
    let old_donors = replay_halo_donors(halo_class_window(bakes, x, y, args.zoom));
    let (geometry, populations) =
        count_donor_geometry(centre.classes(), &old_donors, centre.donors());
    let mut layers = Vec::with_capacity(args.layers.len());
    if let (Some(old_root), Some(new_root)) = (&args.old_root, &args.new_root) {
        for layer in &args.layers {
            let old = read_hm3_or_nodata(old_root, layer, args.zoom, x, y)?;
            let new = read_hm3_or_nodata(new_root, layer, args.zoom, x, y)?;
            let mut drift = [ByteDrift::default(); 3];
            for ((population, &old_byte), &new_byte) in populations.iter().zip(&old).zip(&new) {
                drift[*population as usize].observe(old_byte, new_byte);
            }
            layers.push(drift);
        }
    }
    Ok(TileCensus {
        r4,
        x,
        y,
        vector_mode: true,
        geometry,
        layers,
    })
}

/// All tiles of one R4 cell, sorted by (x, y). Classes are baked once per window
/// member of the cell (a neighbour shared by several centre tiles is baked once),
/// with the SAME ring obstacle set and receiver lattice the painters use.
fn census_cell(args: &Args, rasters: &RealRasters, r4: u64) -> Result<Vec<TileCensus>> {
    let cell = CellIndex::try_from(r4).context("invalid R4 cell")?;
    let tiles = region_tiles(r4, args.zoom);
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let layer_rows = if args.old_root.is_some() {
        args.layers.len()
    } else {
        0
    };
    let obstacles = ObstacleData::load_for_r4s(&args.h3r4_dir, r4, &ring)
        .with_context(|| format!("load obstacles R4 {r4:015x}"))?;
    let Some(set) = obstacles.set() else {
        return Ok(tiles
            .into_iter()
            .map(|(x, y)| TileCensus::raster_fallback(r4, x, y, layer_rows))
            .collect());
    };
    let window: BTreeSet<(u32, u32)> = tiles
        .iter()
        .flat_map(|&(x, y)| {
            (0..9).filter_map(move |ordinal| window_member_xy(x, y, args.zoom, ordinal))
        })
        .collect();
    let bakes: BTreeMap<(u32, u32), InteriorEstimate> = window
        .par_iter()
        .map(|&(x, y)| {
            let tile = FusedTileZ13::build_receiver_altitude_only(args.zoom, x, y, rasters);
            ((x, y), InteriorEstimate::bake(&tile, set))
        })
        .collect();
    let mut rows = tiles
        .par_iter()
        .map(|&(x, y)| census_tile(args, &bakes, r4, x, y))
        .collect::<Result<Vec<_>>>()?;
    rows.sort_by_key(|row| (row.x, row.y));
    Ok(rows)
}

fn tsv_header(out: &mut impl Write) -> std::io::Result<()> {
    write!(
        out,
        "cell\tzoom\tx\ty\tlayer\tmode\tenclosed\told_cross_tile\told_none\tnew_none"
    )?;
    for population in POPULATIONS {
        let p = population.tsv_prefix();
        write!(
            out,
            "\t{p}_pixels\t{p}_differs\t{p}_to_nodata\t{p}_from_nodata\t{p}_finite_both\
             \t{p}_gt0_5db\t{p}_gt1db\t{p}_gt2db\t{p}_gt6db\t{p}_max_abs_db"
        )?;
    }
    writeln!(out)
}

/// The key columns of one TSV row; `xy` is `None` on the `TOTAL` rows.
struct TsvRowHead<'a> {
    cell: &'a str,
    zoom: u8,
    xy: Option<(u32, u32)>,
    layer: &'a str,
    mode: &'a str,
}

fn tsv_row(
    out: &mut impl Write,
    head: &TsvRowHead,
    geometry: &DonorGeometryCounts,
    drift: Option<&[ByteDrift; 3]>,
) -> std::io::Result<()> {
    let (x, y) = head.xy.map_or((String::new(), String::new()), |(x, y)| {
        (x.to_string(), y.to_string())
    });
    write!(
        out,
        "{}\t{}\t{x}\t{y}\t{}\t{}\t{}\t{}\t{}\t{}",
        head.cell,
        head.zoom,
        head.layer,
        head.mode,
        geometry.enclosed,
        geometry.old_cross_tile,
        geometry.old_none,
        geometry.new_none,
    )?;
    match drift {
        Some(drift) => {
            for d in drift {
                write!(
                    out,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
                    d.pixels,
                    d.differs,
                    d.to_nodata,
                    d.from_nodata,
                    d.finite_both,
                    d.over_0_5_db,
                    d.over_1_db,
                    d.over_2_db,
                    d.over_6_db,
                    d.max_abs_db,
                )?;
            }
        }
        None => write!(out, "{}", "\t".repeat(30))?,
    }
    writeln!(out)
}

/// Whole-run totals: geometry over every tile, drift per layer and population.
#[derive(Default)]
struct CensusTotals {
    cells: usize,
    tiles: usize,
    raster_fallback_tiles: usize,
    geometry: DonorGeometryCounts,
    layers: Vec<[ByteDrift; 3]>,
}

impl CensusTotals {
    fn add(&mut self, row: &TileCensus) {
        self.tiles += 1;
        if !row.vector_mode {
            self.raster_fallback_tiles += 1;
        }
        self.geometry.merge(&row.geometry);
        if self.layers.len() < row.layers.len() {
            self.layers
                .resize(row.layers.len(), [ByteDrift::default(); 3]);
        }
        for (total, drift) in self.layers.iter_mut().zip(&row.layers) {
            for (t, d) in total.iter_mut().zip(drift) {
                t.merge(d);
            }
        }
    }
}

fn print_summary(args: &Args, totals: &CensusTotals, elapsed_s: f64) {
    let g = &totals.geometry;
    eprintln!(
        "interior_donor_census zoom={} cells={} tiles={} raster_fallback_tiles={} wall={elapsed_s:.1}s",
        args.zoom, totals.cells, totals.tiles, totals.raster_fallback_tiles,
    );
    eprintln!(
        "geometry enclosed={} old_cross_tile={} old_none={} new_none={}",
        g.enclosed, g.old_cross_tile, g.old_none, g.new_none,
    );
    for (layer, drift) in args.layers.iter().zip(&totals.layers) {
        for (population, d) in POPULATIONS.iter().zip(drift) {
            eprintln!(
                "layer={layer} [{}] pixels={} differs={} to_nodata={} from_nodata={} \
                 finite_both={} >0.5dB={} >1dB={} >2dB={} >6dB={} max_abs_db={:.1}",
                population.label(),
                d.pixels,
                d.differs,
                d.to_nodata,
                d.from_nodata,
                d.finite_both,
                d.over_0_5_db,
                d.over_1_db,
                d.over_2_db,
                d.over_6_db,
                d.max_abs_db,
            );
        }
    }
}

fn run(args: &Args) -> Result<()> {
    let started = Instant::now();
    let mut r4s = read_r4_file(&args.cells_file)?;
    r4s.sort_unstable();
    r4s.dedup();
    if r4s.is_empty() {
        bail!("no R4 cells in {}", args.cells_file.display());
    }
    let rasters = RealRasters::new(&args.prepared_root);

    let mut out: Box<dyn Write> = match &args.tsv_path {
        Some(path) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?,
        )),
        None => Box::new(std::io::BufWriter::new(std::io::stdout())),
    };
    tsv_header(&mut out)?;

    let mut totals = CensusTotals::default();
    for &r4 in &r4s {
        let rows = census_cell(args, &rasters, r4)?;
        totals.cells += 1;
        for row in &rows {
            let cell = format!("{:015x}", row.r4);
            let mut head = TsvRowHead {
                cell: &cell,
                zoom: args.zoom,
                xy: Some((row.x, row.y)),
                layer: "geometry",
                mode: if row.vector_mode { "vector" } else { "raster" },
            };
            tsv_row(&mut out, &head, &row.geometry, None)?;
            for (layer, drift) in args.layers.iter().zip(&row.layers) {
                head.layer = layer;
                tsv_row(&mut out, &head, &row.geometry, Some(drift))?;
            }
            totals.add(row);
        }
        eprintln!(
            "[census] cell {r4:015x} tiles={} raster_fallback={} elapsed={:.1}s",
            rows.len(),
            rows.iter().filter(|row| !row.vector_mode).count(),
            started.elapsed().as_secs_f64(),
        );
    }
    let mut total_head = TsvRowHead {
        cell: "TOTAL",
        zoom: args.zoom,
        xy: None,
        layer: "geometry",
        mode: "",
    };
    tsv_row(&mut out, &total_head, &totals.geometry, None)?;
    for (layer, drift) in args.layers.iter().zip(&totals.layers) {
        total_head.layer = layer;
        tsv_row(&mut out, &total_head, &totals.geometry, Some(drift))?;
    }
    out.flush()?;
    print_summary(args, &totals, started.elapsed().as_secs_f64());
    Ok(())
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(64);
        }
    };
    if let Err(error) = run(&args) {
        eprintln!("interior_donor_census: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outdoor_tile() -> Vec<u8> {
        vec![EnvelopeClass::Outdoor as u8; TILE_CELLS]
    }

    /// A centre tile with a 4-px-wide residential block flush with its west seam.
    fn west_seam_block_tile() -> Vec<u8> {
        let mut classes = outdoor_tile();
        for y in 100..112 {
            for x in 0..4 {
                classes[y * TILE_PX + x] = EnvelopeClass::Residential as u8;
            }
        }
        classes
    }

    /// The retired window donor crosses the west seam for a pixel flush with it
    /// (the outdoor pixel immediately west lives in window member 3), while the
    /// per-tile estimate serves the same pixel from the nearest in-tile façade.
    #[test]
    fn replayed_window_donor_crosses_the_seam_where_the_per_tile_donor_stays_home() {
        let centre = west_seam_block_tile();
        let outdoor = outdoor_tile();
        let window: [&[u8]; 9] = std::array::from_fn(|ordinal| {
            if ordinal == CENTRE_TILE_ORDINAL {
                centre.as_slice()
            } else {
                outdoor.as_slice()
            }
        });
        let old = replay_halo_donors(window);
        assert_eq!(old.len(), TILE_CELLS);
        // Window frame: the centre tile spans x,y ∈ [512, 1024); the pixel one
        // step west of the seam is window x = 511 in member 3.
        let window_ordinal = |x: usize, y: usize| (y * TILE_PX * 3 + x) as u32;
        assert_eq!(
            old[105 * TILE_PX],
            window_ordinal(TILE_PX - 1, TILE_PX + 105)
        );
        assert_eq!(window_tile_ordinal(old[105 * TILE_PX]), 3);
        // Deep inside the block the in-tile façade at x = 4 is closer than the seam.
        assert_eq!(
            old[105 * TILE_PX + 3],
            window_ordinal(TILE_PX + 4, TILE_PX + 105)
        );
        assert_eq!(
            window_tile_ordinal(old[105 * TILE_PX + 3]),
            CENTRE_TILE_ORDINAL
        );
        assert_eq!(
            old[105 * TILE_PX + 200],
            NO_DONOR,
            "outdoor pixels are not queries"
        );

        let new = InteriorEstimate::from_classes(centre.clone());
        assert_eq!(new.donors()[105 * TILE_PX], (105 * TILE_PX + 4) as u32);
        assert_eq!(new.donors()[105 * TILE_PX + 3], (105 * TILE_PX + 4) as u32);

        let (counts, populations) = count_donor_geometry(&centre, &old, new.donors());
        assert_eq!(counts.enclosed, 4 * 12);
        // Column 0 is 1 px from the seam, nearer than any in-tile façade on every
        // row (the block's outdoor rows above and below are ≥ 1 px away and a tie
        // goes to the smaller window x, i.e. the seam): 12 pixels. Column 1 is 2 px
        // from the seam and 3 px from x = 4, but on the block's first and last row
        // the outdoor row above/below is only 1 px away: 10 pixels. Columns 2 and 3
        // are nearer x = 4 than the seam.
        assert_eq!(counts.old_cross_tile, 12 + 10);
        assert_eq!(counts.old_none, 0);
        assert_eq!(counts.new_none, 0);
        assert_eq!(populations[105 * TILE_PX], Population::EnclosedOldCrossTile);
        assert_eq!(populations[105 * TILE_PX + 3], Population::EnclosedOther);
        assert_eq!(populations[105 * TILE_PX + 200], Population::Outdoor);
    }

    /// Beyond the world edge the retired Pass A supplied an enclosed raster, so the
    /// same seam pixel stayed in-tile there — and `halo_class_window` reproduces
    /// that substitution from the bake cache.
    #[test]
    fn world_edge_window_member_is_never_a_donor_site() {
        let zoom = 3;
        let mut bakes = BTreeMap::new();
        bakes.insert(
            (0, 4),
            InteriorEstimate::from_classes(west_seam_block_tile()),
        );
        for ordinal in 0..9 {
            if let Some(member) = window_member_xy(0, 4, zoom, ordinal) {
                bakes
                    .entry(member)
                    .or_insert_with(|| InteriorEstimate::from_classes(outdoor_tile()));
            }
        }
        assert_eq!(window_member_xy(0, 4, zoom, 3), None);
        let window = halo_class_window(&bakes, 0, 4, zoom);
        assert!(window[3]
            .iter()
            .all(|&class| class == EnvelopeClass::Default as u8));
        let old = replay_halo_donors(window);
        assert_eq!(
            old[105 * TILE_PX],
            ((TILE_PX + 105) * TILE_PX * 3 + TILE_PX + 4) as u32
        );
        assert_eq!(window_tile_ordinal(old[105 * TILE_PX]), CENTRE_TILE_ORDINAL);
    }

    #[test]
    fn byte_drift_histogram_counts_nodata_transitions_and_magnitudes() {
        let mut drift = ByteDrift::default();
        drift.observe(100, 100);
        drift.observe(100, NO_DATA);
        drift.observe(NO_DATA, 90);
        drift.observe(NO_DATA, NO_DATA);
        drift.observe(100, 101); // 0.5 dB: not > 0.5
        drift.observe(100, 103); // 1.5 dB
        drift.observe(100, 80); // 10 dB
        assert_eq!(drift.pixels, 7);
        assert_eq!(drift.differs, 5);
        assert_eq!(drift.to_nodata, 1);
        assert_eq!(drift.from_nodata, 1);
        assert_eq!(drift.finite_both, 4);
        assert_eq!(drift.over_0_5_db, 2);
        assert_eq!(drift.over_1_db, 2);
        assert_eq!(drift.over_2_db, 1);
        assert_eq!(drift.over_6_db, 1);
        assert_eq!(drift.max_abs_db, 10.0);
    }
}
