"""Stream-reduce official 1 m terrain to retained 5 m grids with a checksum manifest.

A 1 m tile covers about 600 native samples per 1 arc-second node. Averaging each
5 x 5 m block first and letting the producer average 5 m to 1 arc-second keeps
one averaging path and loses nothing measurable (the reduction error is checked
against direct 1 m averaging on pilot squares). Raw 1 m bytes are retained only
for small providers; otherwise the manifest below is the audit trail and the
raw bytes are deleted after the derived grid verifies.
"""
import json
from pathlib import Path
import warnings
import zipfile

import numpy as np
from osgeo import gdal, osr

from terrain_io import digest, provenance, publish_bytes, publish_json, publish_source_json

gdal.UseExceptions()
osr.UseExceptions()

DERIVED_METRES = 5
DERIVED_NODATA = -9999.0


def decode_xyz_lattice(path, spacing, nominal=None):
    """Place a verified regular XYZ lattice without resampling or datum change.

    With nominal=(x0, y0, nx, ny), nodes land on the tile's nominal lattice
    and missing nodes stay nodata instead of failing the tile; nodes off the
    nominal lattice, outside its extent, or landing twice are refused.
    """
    with open(path, encoding='utf-8', errors='replace') as handle:
        first = handle.readline().split()
    try:
        [float(cell) for cell in first]
        skip = 0
    except ValueError:
        skip = 1
    points = np.loadtxt(path, skiprows=skip)
    if points.ndim != 2 or not len(points):
        raise ValueError(f'{path}: no XYZ rows to place')
    if nominal is not None:
        return place_nominal_lattice(path, points, spacing, nominal)
    xs, ys = np.unique(points[:, 0]), np.unique(points[:, 1])[::-1]
    if (len(xs) * len(ys) != len(points) or not np.all(np.diff(xs) == spacing)
            or not np.all(np.diff(ys) == -spacing)):
        raise ValueError(f'{path}: not a complete {spacing} m lattice')
    rows = np.rint((ys[0] - points[:, 1]) / spacing).astype(int)
    cols = np.rint((points[:, 0] - xs[0]) / spacing).astype(int)
    values = np.full((len(ys), len(xs)), np.nan)
    values[rows, cols] = points[:, 2]
    if not np.isfinite(values).all():
        raise ValueError(f'{path}: missing or duplicated XYZ nodes')
    return xs, ys, values


def place_nominal_lattice(path, points, spacing, nominal):
    """Grid XYZ nodes onto a nominal tile extent, leaving gaps as nodata."""
    x0, y0, nx, ny = nominal
    xs = x0 + spacing / 2 + np.arange(nx) * spacing
    ys = y0 + ny * spacing - spacing / 2 - np.arange(ny) * spacing
    cols = np.rint((points[:, 0] - xs[0]) / spacing).astype(int)
    rows = np.rint((ys[0] - points[:, 1]) / spacing).astype(int)
    on_grid = (np.abs(points[:, 0] - (xs[0] + cols * spacing)) <= spacing * 0.01
               ) & (np.abs(points[:, 1] - (ys[0] - rows * spacing)) <= spacing * 0.01)
    if not on_grid.all():
        raise ValueError(f'{path}: XYZ nodes off the nominal {spacing} m lattice')
    if (rows < 0).any() or (rows >= ny).any() or (cols < 0).any() or (cols >= nx).any():
        raise ValueError(f'{path}: XYZ nodes outside the nominal tile extent')
    if len(np.unique(rows * nx + cols)) != len(points):
        raise ValueError(f'{path}: duplicated XYZ nodes')
    values = np.full((ny, nx), np.nan)
    values[rows, cols] = points[:, 2]
    return xs, ys, values


def block_average(values, factor):
    """Exact area mean over factor x factor blocks; all-nodata blocks stay nodata."""
    rows, cols = values.shape
    if rows % factor or cols % factor:
        raise ValueError(f'{values.shape} is not divisible by {factor}')
    blocks = values.reshape(rows // factor, factor, cols // factor, factor)
    # All-nodata blocks are water or coverage edges; their NaN mean is the result.
    with warnings.catch_warnings():
        warnings.simplefilter('ignore', RuntimeWarning)
        mean = np.nanmean(blocks, axis=(1, 3))
    return mean


def block_average_offset(values, factor):
    """Exact area mean for half-cell-offset (k*factor+1) grids.

    Point-registered 1 m tiles (RP's 1001-node revision) centre on integer
    metres, half a cell off the derived lattice. Each derived cell straddles
    factor+1 source cells with half weights on the shared edges; the weights
    sum to factor**2, the same support as block_average. All-nodata blocks
    stay nodata.
    """
    rows, cols = values.shape
    if (rows - 1) % factor or (cols - 1) % factor:
        raise ValueError(f'{values.shape} is not one-over divisible by {factor}')
    out_rows, out_cols = (rows - 1) // factor, (cols - 1) // factor
    mean = np.full((out_rows, out_cols), np.nan)
    edge = np.ones(factor + 1)
    edge[0] = edge[-1] = 0.5
    weights = np.outer(edge, edge)
    for row in range(out_rows):
        part = values[row * factor:row * factor + factor + 1, :]
        for col in range(out_cols):
            block = part[:, col * factor:col * factor + factor + 1]
            finite = np.isfinite(block)
            if not finite.any():
                continue
            picked = weights[finite]
            mean[row, col] = float(np.sum(block[finite] * picked) / np.sum(picked))
    return mean


def write_derived_grid(target, west, north, step, values, epsg):
    """Write one Float32 DEFLATE grid with pixel-centre convention documented."""
    target = Path(target)
    if target.exists():
        return
    rows, cols = values.shape
    driver = gdal.GetDriverByName('GTiff')
    dataset = driver.Create(str(target), cols, rows, 1, gdal.GDT_Float32,
                            options=['COMPRESS=DEFLATE', 'TILED=YES'])
    crs = osr.SpatialReference()
    crs.ImportFromEPSG(epsg)
    dataset.SetProjection(crs.ExportToWkt())
    # Input points are cell centres; the edge sits half a step outside.
    dataset.SetGeoTransform([west - step / 2, step, 0, north + step / 2, 0, -step])
    band = dataset.GetRasterBand(1)
    band.SetNoDataValue(DERIVED_NODATA)
    filled = np.where(np.isfinite(values), values, DERIVED_NODATA)
    band.WriteArray(filled.astype(np.float32))
    dataset = None


def reduce_geotiff(source, target, src_nodata, factor=DERIVED_METRES, epsg=None):
    """Area-average a north-up 1 m GeoTIFF onto the aligned derived grid."""
    dataset = gdal.Open(str(source))
    if dataset.RasterCount != 1:
        raise ValueError(f'{source}: expected one band')
    transform = dataset.GetGeoTransform()
    if transform[2] or transform[4] or transform[1] <= 0 or transform[5] >= 0:
        raise ValueError(f'{source}: requires a north-up pixel grid')
    step = transform[1]
    if not np.isclose(-transform[5], step, rtol=1e-9, atol=0):
        raise ValueError(f'{source}: non-square pixels {transform[1:6:4]}')
    values = dataset.GetRasterBand(1).ReadAsArray().astype(np.float64)
    nodata = src_nodata if src_nodata is not None else dataset.GetRasterBand(1).GetNoDataValue()
    if nodata is not None and np.isfinite(nodata):
        values[values == nodata] = np.nan
    # Block averaging needs whole blocks from the tile's west/north edges.
    factor_cells = int(round(factor / step))
    if not np.isclose(factor_cells * step, factor, rtol=1e-9, atol=0):
        raise ValueError(f'{source}: {step} m cells do not divide {factor} m')
    rows, cols = values.shape
    if rows % factor_cells == 0 and cols % factor_cells == 0:
        mean = block_average(values, factor_cells)
        west, north = transform[0] + factor / 2, transform[3] - factor / 2
    elif (rows - 1) % factor_cells == 0 and (cols - 1) % factor_cells == 0:
        # Half-cell-offset tiles inset their derived edges by half a source
        # step; the inset edges must land on the derived lattice, else refuse.
        west_edge, north_edge = transform[0] + step / 2, transform[3] - step / 2
        for edge, label in ((west_edge, 'west'), (north_edge, 'north')):
            rest = edge % factor
            if not (np.isclose(rest, 0, atol=1e-6) or np.isclose(rest, factor, atol=1e-6)):
                raise ValueError(f'{source}: offset {label} edge {edge} misses the {factor} m lattice')
        mean = block_average_offset(values, factor_cells)
        west, north = west_edge + factor / 2, north_edge - factor / 2
    else:
        raise ValueError(f'{source}: {values.shape} fits neither the {factor_cells}-cell nor the offset lattice')
    # Outer edges stay fixed; the first derived centre sits half a step inside.
    if epsg is None:
        crs = osr.SpatialReference(wkt=dataset.GetProjection())
        epsg = int(crs.GetAttrValue('AUTHORITY', 1)) if crs.GetAttrValue('AUTHORITY', 0) == 'EPSG' else None
    if epsg is None:
        raise ValueError(f'{source}: cannot read an EPSG code from its projection')
    write_derived_grid(target, west, north, factor, mean, epsg)
    return dict(rows=int(mean.shape[0]), columns=int(mean.shape[1]),
                valid_fraction=float(np.mean(np.isfinite(mean))))


def reduce_xyz(path, target, spacing, epsg, factor=DERIVED_METRES, src_nodata=-9999.0,
               zone_prefix=None, corner_registered=False, nominal=None):
    """Decode a verified XYZ lattice, then area-average it onto the derived grid."""
    if nominal is not None and corner_registered:
        raise ValueError('nominal placement assumes centre-registered XYZ nodes')
    xs, ys, values = decode_xyz_lattice(path, spacing, nominal=nominal)
    if zone_prefix is not None and xs.min() >= zone_prefix:
        # Bremerhaven tiles prefix eastings with the UTM zone; EPSG:25832 drops it.
        xs = xs - zone_prefix
    # XYZ carries no nodata tag; the AdV void value is void, never -9999 m terrain.
    values[values == src_nodata] = np.nan
    if spacing == factor:
        mean, step = values, spacing
    elif factor % spacing == 0:
        mean, step = block_average(values, factor // spacing), factor
    else:
        raise ValueError(f'{path}: {spacing} m lattice does not divide {factor} m')
    if corner_registered:
        west, north = float(xs[0]) + step / 2, float(ys[0]) + spacing - step / 2
    else:
        # The lattice edge sits half a source step outside the first centre.
        west = float(xs[0]) - spacing / 2 + step / 2
        north = float(ys[0]) + spacing / 2 - step / 2
    write_derived_grid(target, west, north, step, mean, epsg)
    return dict(rows=int(mean.shape[0]), columns=int(mean.shape[1]),
                valid_fraction=float(np.mean(np.isfinite(mean))))


def normalize_grid(raw, derived, assign_epsg=None):
    """Compress an already-5 m grid, remap its nodata and verify or assign its CRS.

    gdal.Translate's nodata flag only retags; the source void value (0 in
    UInt16 coverages, 3.4e38 in Float32 ones) is remapped to the derived
    nodata explicitly so voids never become 0 m or 3.4e38 m terrain. A grid
    without any projection (the Brandenburg WCS answers carry none) is
    assigned the documented request CRS; a grid whose embedded EPSG
    contradicts the expectation is refused. Samples never move.
    """
    raw, derived = Path(raw), Path(derived)
    if derived.exists():
        return False
    if assign_epsg is not None:
        expected = osr.SpatialReference()
        expected.ImportFromEPSG(assign_epsg)
    else:
        expected = None
    probe = gdal.Open(str(raw))
    if probe.RasterCount != 1:
        raise ValueError(f'{raw}: expected one band')
    wkt = probe.GetProjection()
    band = probe.GetRasterBand(1)
    src_nodata = band.GetNoDataValue()
    if src_nodata is None:
        raise ValueError(f'{raw}: no source nodata declared; refusing silent void handling')
    raw_crs = osr.SpatialReference(wkt=wkt) if wkt else None
    if raw_crs is None or not raw_crs.IsProjected():
        if expected is None:
            raise ValueError(f'{raw}: no projected CRS to verify against')
        output_srs = expected.ExportToWkt()
    else:
        authority = raw_crs.GetAuthorityCode('PROJCS')
        if expected is not None and authority != str(assign_epsg):
            raise ValueError(f'{raw}: horizontal CRS EPSG:{authority}, expected EPSG:{assign_epsg}')
        output_srs = None
    probe = None
    options = ['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2']
    dataset = gdal.Open(str(raw))
    kwargs = dict(format='GTiff', outputType=gdal.GDT_Float32, noData=float(DERIVED_NODATA),
                  creationOptions=options, resampleAlg=gdal.GRA_NearestNeighbour)
    if output_srs is not None:
        kwargs['outputSRS'] = output_srs
    translated = gdal.Translate(str(derived), dataset, **kwargs)
    dataset = None
    if translated is None:
        raise ValueError(f'{raw}: the 5 m grid is unreadable')
    translated = None
    if not np.isclose(float(src_nodata), DERIVED_NODATA):
        # The retag above left the void pixels untouched; remap them now.
        fixed = gdal.Open(str(derived), gdal.GA_Update)
        values = fixed.GetRasterBand(1).ReadAsArray()
        values[values == np.float32(src_nodata)] = np.float32(DERIVED_NODATA)
        fixed.GetRasterBand(1).WriteArray(values)
        fixed = None
    return True


def derived_complete(path):
    """A derived grid counts as done only with its provenance sidecar.

    The sidecar is written after the grid verifies, so a grid without one
    is a crash remnant and must be decoded again, never trusted.
    """
    return Path(path).exists() and Path(str(path) + '.provenance.json').exists()


def clean_remnants(path):
    """Remove a grid and its sidecar so a decode always starts from nothing."""
    for candidate in (Path(path), Path(str(path) + '.provenance.json')):
        if candidate.exists():
            candidate.unlink()


def derive_provenance(raw_path, derived_path, method):
    """Attach the derived grid to its verified raw parent (Bavaria pattern)."""
    record = provenance(raw_path)
    publish_json(str(derived_path) + '.provenance.json',
                 dict(record, sha256=digest(derived_path), bytes=Path(derived_path).stat().st_size,
                      parent_sha256=record['sha256'], notes=method))


def assert_crs(path, horizontal_epsg, vertical_epsg=None):
    """Refuse any grid whose embedded CRS contradicts the provider statement."""
    dataset = gdal.Open(str(path))
    crs = osr.SpatialReference(wkt=dataset.GetProjection())
    dataset = None
    if not crs.IsProjected():
        raise ValueError(f'{path}: expected a projected CRS')
    authority = crs.GetAuthorityName('PROJCS')
    code = crs.GetAuthorityCode('PROJCS')
    if authority != 'EPSG' or code != str(horizontal_epsg):
        raise ValueError(f'{path}: horizontal CRS {authority}:{code}, expected EPSG:{horizontal_epsg}')
    vertical = crs.GetAuthorityCode('VERT_CS')
    if vertical_epsg is not None and vertical is not None and vertical != str(vertical_epsg):
        raise ValueError(f'{path}: vertical CRS EPSG:{vertical}, expected EPSG:{vertical_epsg}')
    return vertical


def read_zip_member(data, suffix):
    """Return the single archive member with the given suffix (names must agree)."""
    with zipfile.ZipFile(data) as archive:
        names = [n for n in archive.namelist() if n.endswith(suffix)]
        if len(names) != 1:
            raise ValueError(f'expected one {suffix} member, found {len(names)}')
        with archive.open(names[0]) as member:
            return names[0], member.read()


def manifest_entry(source_url, raw_path, derived_path, method):
    """One checksum-manifest row linking a raw source to its retained derivative."""
    raw = Path(raw_path)
    derived = Path(derived_path)
    return dict(url=source_url, raw_bytes=raw.stat().st_size,
                raw_sha256=digest(raw), derived=str(derived.name),
                derived_sha256=digest(derived), derived_bytes=derived.stat().st_size,
                method=method)


def write_manifest(path, entries):
    publish_json(path, dict(entries=entries, count=len(entries),
                            raw_bytes=sum(e['raw_bytes'] for e in entries),
                            derived_bytes=sum(e['derived_bytes'] for e in entries)))


def load_manifest(path):
    manifest = json.loads(Path(path).read_text())
    if manifest['count'] != len(manifest['entries']):
        raise ValueError(f'checksum manifest miscounts its entries: {path}')
    return manifest


def country_epoch(entries, default):
    """One honest group epoch: the dated range, or the mosaic label, never invented."""
    dated = sorted(e['epoch'][4:] for e in entries if e.get('epoch', '').startswith('ALS '))
    unknown = any(e.get('epoch', '').startswith('unknown') for e in entries)
    if not dated:
        return 'unknown ALS epoch' if unknown else default
    label = f'ALS {dated[0]}' if dated[0] == dated[-1] else f'ALS {dated[0]} to {dated[-1]}'
    return label + ' (plus undated tiles)' if unknown else label


def publish_country_sources(root, provider, entries, horizontal_epsg, vertical_epsg,
                            group, default_epoch):
    """Write manifest-ready source specs: absolute paths, one epoch per group.

    Per-tile epochs stay in the stream manifest; the producer mosaics one
    group at a time and requires a uniform epoch per group.
    """
    kept = sorted((e for e in entries if e.get('derived')), key=lambda e: e['derived'])
    epoch = country_epoch(kept, default_epoch)
    value = [dict(path=str(Path(root).resolve() / provider / e['derived']),
                  horizontal_crs=f'EPSG:{horizontal_epsg}', vertical_crs=vertical_epsg,
                  epoch=epoch, role='national', group=group)
             for e in kept]
    sources = Path(root) / provider / 'country-sources.json'
    try:
        publish_source_json(root, sources, value)
    except ValueError:
        sources.unlink()
        publish_source_json(root, sources, value)
    return value


def load_journal(path):
    """Read the append-only fetch journal; each line is one manifest entry."""
    entries = []
    if Path(path).exists():
        with open(path, encoding='utf-8') as handle:
            for line in handle:
                if line.strip():
                    entries.append(json.loads(line))
    done = set(e.get('derived') for e in entries if e.get('derived'))
    return entries, done


def append_journal(path, entry, lock=None):
    """Append one entry; the journal is the resumable truth, the manifest a copy."""
    line = json.dumps(entry, sort_keys=True) + '\n'
    if lock is not None:
        with lock:
            with open(path, 'a', encoding='utf-8') as handle:
                handle.write(line)
    else:
        with open(path, 'a', encoding='utf-8') as handle:
            handle.write(line)
