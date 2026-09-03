// Device construction of the C2 receiver terrain horizon.
#pragma once

#if !defined(BUILDING_ENV_TERRAIN_SAMPLES) || !defined(TERRAIN_MARCH_SAMPLES) || \
    !defined(TERRAIN_SECTORS) || !defined(TERRAIN_BANDS) || !defined(TAN_SCALE_D) || \
    !defined(TERRAIN_RANGE_SCALE_D)
#error "airborne terrain horizon constants must be injected by build.rs"
#endif

#define TERRAIN_SAMPLES_PER_RECEIVER (TERRAIN_SECTORS * TERRAIN_MARCH_SAMPLES)

// Per receiver row and sample, everything the march derives from the row alone: the
// longitude offset in degrees (east metres over the row's metres per degree), the sample
// latitude's halo lattice row, and its inner pixel row inside the tile bbox (or -1). The
// same f64 expressions the march evaluated per sample, formed once per row and shared by
// its 512 receivers.
extern "C" __global__ void airborne_terrain_sample_tables(
    const unsigned long long* __restrict__ environment,
    const double* __restrict__ receiver_lat_lon,
    const double* __restrict__ tile_bbox,
    double* __restrict__ east_deg,
    double* __restrict__ row_rf,
    int* __restrict__ row_idx)
{
    long long item = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    if (item >= (long long)TPX * TERRAIN_SAMPLES_PER_RECEIVER) return;
    int py = (int)(item / TERRAIN_SAMPLES_PER_RECEIVER);
    int sample = (int)(item % TERRAIN_SAMPLES_PER_RECEIVER);
    const double* samples =
        reinterpret_cast<const double*>(environment[BUILDING_ENV_TERRAIN_SAMPLES]);
    east_deg[item] = samples[sample * 4 + 2] / receiver_lat_lon[2 * TPX + py];
    double sample_lat = fmin(fmax(receiver_lat_lon[py] + samples[sample * 4 + 1], -90.0), 90.0);
    row_rf[item] = building_dem_lattice_row(environment, sample_lat);
    row_idx[item] = building_tile_row(tile_bbox, sample_lat);
}

extern "C" __global__ void airborne_terrain_horizon_build(
    const unsigned long long* __restrict__ environment,
    const double* __restrict__ receiver_lat_lon,
    const float* __restrict__ receiver_altitude,
    const float* __restrict__ inner_elevation,
    const double* __restrict__ tile_bbox,
    const unsigned int* __restrict__ pixel_of_record,
    const double* __restrict__ east_deg,
    const double* __restrict__ row_rf,
    const int* __restrict__ row_idx,
    int records,
    unsigned int* __restrict__ entries)
{
    long long worker = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    long long count = (long long)records * TERRAIN_SECTORS;
    if (worker >= count) return;
    int record = (int)(worker % records);
    int sector = (int)(worker / records);
    unsigned int pixel = pixel_of_record[record];
    int py = pixel >> TPX_SHIFT;
    int px = pixel & TPX_MASK;
    double receiver_lon = receiver_lat_lon[TPX + px];
    double receiver_alt_m = (double)receiver_altitude[pixel];
    const double* samples =
        reinterpret_cast<const double*>(environment[BUILDING_ENV_TERRAIN_SAMPLES]);
    long long row_base = (long long)py * TERRAIN_SAMPLES_PER_RECEIVER + sector * TERRAIN_MARCH_SAMPLES;
    const double* row_east_deg = east_deg + row_base;
    const double* sector_row_rf = row_rf + row_base;
    const int* sector_row_idx = row_idx + row_base;
    double best_tangent[TERRAIN_BANDS];
    unsigned short best_range_m[TERRAIN_BANDS];
    for (int band = 0; band < TERRAIN_BANDS; band++) {
        best_tangent[band] = -3.4028234663852886e38;
        best_range_m[band] = 0;
    }

    unsigned long long sample_base =
        (unsigned long long)sector * TERRAIN_MARCH_SAMPLES * 4;
    for (int sample_index = 0; sample_index < TERRAIN_MARCH_SAMPLES; sample_index++) {
        const double* sample = samples + sample_base + sample_index * 4;
        double sample_lon = receiver_lon + row_east_deg[sample_index];
        if (sample_lon >= 180.0) sample_lon -= 360.0;
        if (sample_lon < -180.0) sample_lon += 360.0;
        // `building_tile_elevation` with its latitude half read from the row tables.
        int inner_row = sector_row_idx[sample_index];
        double elevation_m;
        if (inner_row >= 0 && sample_lon >= tile_bbox[2] && sample_lon <= tile_bbox[3]) {
            double lon_fraction = (sample_lon - tile_bbox[2]) / (tile_bbox[3] - tile_bbox[2]);
            int inner_col = (int)floor(fmin(fmax(lon_fraction * TPX, 0.0), (double)(TPX - 1)));
            elevation_m = (double)inner_elevation[inner_row * TPX + inner_col];
        } else {
            elevation_m = building_dem_elevation_at(
                environment, sector_row_rf[sample_index],
                building_dem_lattice_col(environment, sample_lon));
        }
        double tangent = (elevation_m - receiver_alt_m) / sample[0];
        int band = (int)sample[3];
        if (tangent > best_tangent[band]) {
            best_tangent[band] = tangent;
            best_range_m[band] = (unsigned short)ceil(
                sample[0] * TERRAIN_RANGE_SCALE_D);
        }
    }

    for (int band = 0; band < TERRAIN_BANDS; band++) {
        long long tangent_q = llround(best_tangent[band] * TAN_SCALE_D);
        tangent_q = tangent_q < -32768 ? -32768 : (tangent_q > 32767 ? 32767 : tangent_q);
        unsigned int packed = ((unsigned int)(unsigned short)(short)tangent_q << 16)
            | (unsigned int)best_range_m[band];
        unsigned long long entry =
            ((unsigned long long)sector * TERRAIN_BANDS + band)
            * (unsigned long long)records + record;
        entries[entry] = packed;
    }
}

extern "C" __global__ void airborne_terrain_horizon_global_max(
    int records,
    const unsigned int* __restrict__ entries,
    float* __restrict__ max_sin_sq)
{
    int record = (int)((long long)blockIdx.x * blockDim.x + threadIdx.x);
    if (record >= records) return;
    int max_tangent_q = -32768;
    for (int entry = 0; entry < TERRAIN_SECTORS * TERRAIN_BANDS; entry++) {
        unsigned int packed = entries[(unsigned long long)entry * records + record];
        if ((unsigned short)packed == 0) continue;
        max_tangent_q = max(max_tangent_q, (int)(short)(packed >> 16));
    }
    double max_tangent = (double)max_tangent_q / TAN_SCALE_D;
    max_sin_sq[record] = max_tangent > 0.0
        ? (float)(max_tangent * max_tangent / (1.0 + max_tangent * max_tangent))
        : 0.0f;
}

// Acceptance probe for the range-packing contract. The source is deliberately
// between the true edge range and the old nearest-metre value; the conservative
// ceiling must leave the edge out of the receiver-to-source interval, so the
// real screening query returns zero rather than only checking the packed value.
extern "C" __global__ void airborne_terrain_horizon_range_quantization_probe(
    float true_range_m, float source_range_m, float* dz_out)
{
    if (blockIdx.x != 0 || threadIdx.x != 0) return;
    unsigned int entries[TERRAIN_BANDS] = {};
    unsigned int range_q = (unsigned int)ceil(
        (double)true_range_m * TERRAIN_RANGE_SCALE_D);
    unsigned int tangent_q = (unsigned int)llround(TAN_SCALE_D);
    entries[0] = (tangent_q << 16) | range_q;
    dz_out[0] = range_max_screening_db(
        entries, 1, 0, 0, TERRAIN_BANDS, (float)TERRAIN_RANGE_SCALE_D,
        source_range_m, 0.0f, false, false);
}
