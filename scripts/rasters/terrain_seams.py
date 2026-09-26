"""Feather grouped national coverage and gate only source-selection artifacts, not natural slopes."""
import json
from pathlib import Path
from terrain_io import digest
import numpy as np
from scipy.ndimage import distance_transform_edt

# The reader contract permits 0.5 m artificial steps, including two 0.1 m rounding errors.
MAX_ARTIFICIAL_STEP_M = .5
QUANTIZATION_STEP_BUDGET_M = .2
SELECTION_BUDGET_M = MAX_ARTIFICIAL_STEP_M - QUANTIZATION_STEP_BUDGET_M
SLOPE_EXEMPT_RATIO = .1
EXEMPT_CAP_M = 1.


def expanded(window, halo):
    return dict(window, north_node=window['north_node'] + halo,
                west_node=window['west_node'] - halo, rows=window['rows'] + 2 * halo,
                columns=window['columns'] + 2 * halo)


def feather(base, national, halo):
    """A finite halo makes each node independent of the square being produced."""
    if halo < 2:
        raise ValueError('national coverage requires a feather halo of at least two nodes')
    valid = np.isfinite(national)
    # Explicit outside padding matters when every node in the processing window is national.
    distance = distance_transform_edt(np.pad(valid, 1))[1:-1, 1:-1]
    weight = np.minimum(np.maximum(distance - 1, 0) / halo, 1)
    if np.any(valid & ~np.isfinite(base)):
        raise ValueError('national feathering requires a finite fallback throughout its coverage')
    difference = np.where(valid, national - base, 0)
    return base + weight * difference, weight, difference


def axial_slopes(national, base, axis):
    low = [slice(None), slice(None)]
    high = low.copy()
    low[axis], high[axis] = slice(None, -1), slice(1, None)
    low, high = tuple(low), tuple(high)
    slope = np.abs(base[high] - base[low])
    both = np.isfinite(national[low]) & np.isfinite(national[high])
    return np.maximum(slope, np.where(both, np.abs(national[high] - national[low]), 0.))


def relief(slopes, axis):
    """Largest natural step touching each edge: its own axial slope and the four
    cross-axis steps at its endpoints. A cliff running along the edge still shows
    its face across the edge, so along-strike ramp edges exempt like
    across-strike ones (Tenerife's west cliff); flat-terrain edges keep a zero
    cross slope and still trip."""
    own, cross = slopes[axis], slopes[1 - axis]
    if axis == 0:
        padded = np.pad(cross, ((0, 0), (1, 1)))
        touching = np.maximum.reduce([padded[:-1, :-1], padded[:-1, 1:], padded[1:, :-1], padded[1:, 1:]])
    else:
        padded = np.pad(cross, ((1, 1), (0, 0)))
        touching = np.maximum.reduce([padded[:-1, :-1], padded[1:, :-1], padded[:-1, 1:], padded[1:, 1:]])
    return np.maximum(own, touching)


def core_relief(national, base, halo):
    """Per-edge natural relief on the gated core, matching artificial_steps' crop."""
    slopes = [axial_slopes(national, base, axis) for axis in (0, 1)]
    cores = []
    for axis in (0, 1):
        slope = relief(slopes, axis)
        cores.append(slope[halo - 1:slope.shape[0] - halo + 1,
                           halo - 1:slope.shape[1] - halo + 1])
    return cores


def hides_in_relief(steps, natural):
    """Over-budget ramp edges hidden under a tenth of the local relief (cap 1 m)."""
    return ((steps > SELECTION_BUDGET_M)
            & (steps <= np.minimum(SLOPE_EXEMPT_RATIO * natural, EXEMPT_CAP_M)))


def artificial_steps(weight, difference, halo, national, base):
    """Decompose Δ(w*d) = mean(w)*Δd + mean(d)*Δw; gate the selection-weight term.

    Returns the group statistics plus the gated per-edge steps on both axes, so the
    caller stacks groups edge by edge: maxima stack only where transitions colocate.
    A ramp under a tenth of the local relief hides in the terrain (cliffs resolve
    30 m against 5 m there), so steep-slope edges exempt up to a metre; anything
    beyond that, and every flat edge over budget, still trips.
    """
    naturals = core_relief(national, base, halo)
    steps = []
    gated = []
    stacks = []
    exempted = 0
    max_exempted = 0.
    for axis in (0, 1):
        low = [slice(None), slice(None)]
        high = low.copy()
        low[axis], high[axis] = slice(None, -1), slice(1, None)
        low, high = tuple(low), tuple(high)
        # At coverage limits, continue the last available residual across the boundary.
        # Weight is zero there; this affects only the conservative error bound.
        bound = np.maximum(np.abs(difference[low]), np.abs(difference[high]))
        induced = np.abs(np.diff(weight, axis=axis)) * bound
        core = induced[halo - 1:induced.shape[0] - halo + 1,
                       halo - 1:induced.shape[1] - halo + 1]
        natural = naturals[axis]
        fold = np.isfinite(core) & (core > 0)
        selected = core[fold]
        steps.append(selected)
        exempt = hides_in_relief(selected, natural[fold])
        exempted += int(exempt.sum())
        if exempt.any():
            max_exempted = max(max_exempted, float(selected[exempt].max()))
        gated.append(selected[~exempt])
        gated_map = np.zeros_like(core, dtype=np.float32)
        gated_map[fold] = np.where(exempt, 0., selected).astype(np.float32)
        stacks.append(gated_map)
    nonzero = np.concatenate(steps)
    gated_all = np.concatenate(gated)
    maximum = float(nonzero.max(initial=0))
    gated_maximum = float(gated_all.max(initial=0))
    return dict(evaluated_transition_edges=int(len(nonzero)),
                maximum_selection_step_m=maximum,
                p95_selection_step_m=float(np.percentile(nonzero, 95)) if len(nonzero) else 0,
                quantization_budget_m=QUANTIZATION_STEP_BUDGET_M,
                maximum_artificial_step_bound_m=gated_maximum + QUANTIZATION_STEP_BUDGET_M,
                maximum_gated_step_m=gated_maximum, selection_exempt_edges=exempted,
                maximum_exempted_step_m=max_exempted), stacks[0], stacks[1]


def require_seam_gate(statistics):
    if statistics['maximum_artificial_step_bound_m'] > MAX_ARTIFICIAL_STEP_M + 1e-10:
        raise ValueError(f'artificial source seam exceeds {MAX_ARTIFICIAL_STEP_M} m: {statistics}')

def verify_shared_nodes(path, encoded, window, binary, identity, raster_window, ocean_manifest_sha256=None):
    """Check every already-published neighbour; later neighbours check this square in turn.

    Shared node VALUES are the gate: a neighbour produced under another partial plan is
    compared all the same and recorded, so the post-merge rebuild list is explicit.
    Empty ocean files depend only on the reviewed coverage manifest, never on a DEM plan.
    """
    x, y = int(path.parent.parent.name), int(path.parent.name)
    root = path.parents[3]
    maximum, count = 0., 0
    cross_plan = []
    for dx, dy in ((a, b) for a in (-1, 0, 1) for b in (-1, 0, 1) if a or b):
        if not 0 <= y + dy < 512:
            continue
        xx, yy = (x + dx) % 512, y + dy
        neighbour = root / 'z9' / str(xx) / str(yy) / path.name
        sidecar = Path(str(neighbour) + '.provenance.json')
        if not sidecar.exists():
            continue
        record = json.loads(sidecar.read_text())
        if digest(neighbour) != record['sha256']:
            raise ValueError(f'neighbour bytes changed: {neighbour}')
        if neighbour.stat().st_size == 0:
            if not record.get('coverage_verified_ocean'):
                raise ValueError('empty neighbour lacks verified ocean coverage')
            if ocean_manifest_sha256 is not None and record.get('coverage_manifest_sha256') != ocean_manifest_sha256:
                raise ValueError(f'neighbour ocean coverage differs from reviewed manifest: {neighbour}')
        elif record.get('plan_sha256') != identity:
            cross_plan.append([xx, yy])
        other = raster_window(binary, xx, yy)
        if x == 0 and xx == 511:
            other['west_node'] -= 360 * window['nodes_per_degree']
        elif x == 511 and xx == 0:
            other['west_node'] += 360 * window['nodes_per_degree']
        west = max(window['west_node'], other['west_node'])
        east = min(window['west_node'] + window['columns'], other['west_node'] + other['columns'])
        north = min(window['north_node'], other['north_node'])
        south = max(window['north_node'] - window['rows'], other['north_node'] - other['rows'])
        if east <= west or north <= south:
            raise ValueError('adjacent squares lack shared lattice nodes')
        def subset(array, w):
            return array[w['north_node'] - north:w['north_node'] - south,
                         west - w['west_node']:east - w['west_node']]
        current = subset(encoded, window).astype(float)
        if neighbour.stat().st_size == 0:
            if not record.get('coverage_verified_ocean'):
                raise ValueError('empty neighbour lacks verified ocean coverage')
            delta = np.abs(window['dem_offset_m'] + current / window['dem_codes_per_metre'])
        else:
            previous = np.memmap(neighbour, mode='r', dtype='<u2', shape=(other['rows'], other['columns']))
            delta = np.abs(current - subset(previous, other)) / window['dem_codes_per_metre']
        maximum = max(maximum, float(delta.max()))
        count += int(delta.size)
    if maximum > .5:
        raise ValueError(f'shared square edge exceeds 0.5 m: {maximum}')
    return dict(shared_nodes=count, maximum_shared_node_difference_m=maximum,
                cross_plan_neighbours=cross_plan)
