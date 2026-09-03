// Device construction of the C2 receiver terrain horizon.
#pragma once

#if !defined(BUILDING_ENV_TERRAIN_SAMPLES) || !defined(TERRAIN_MARCH_SAMPLES) || \
    !defined(TERRAIN_SECTORS) || !defined(TERRAIN_BANDS) || !defined(TAN_SCALE_D) || \
    !defined(TERRAIN_RANGE_SCALE_D)
#error "airborne terrain horizon constants must be injected by build.rs"
#endif

extern "C" __global__ void airborne_terrain_horizon_build(
    const unsigned long long* __restrict__ environment,
    const double* __restrict__ receiver_lat_lon,
    const float* __restrict__ receiver_altitude,
    const float* __restrict__ inner_elevation,
    const double* __restrict__ tile_bbox,
    const unsigned int* __restrict__ pixel_of_record,
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
    double receiver_lat = receiver_lat_lon[py];
    double receiver_lon = receiver_lat_lon[TPX + px];
    double m_per_deg_lon = receiver_lat_lon[2 * TPX + py];
    double receiver_alt_m = (double)receiver_altitude[pixel];
    const double* samples =
        reinterpret_cast<const double*>(environment[BUILDING_ENV_TERRAIN_SAMPLES]);
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
        double sample_lat = fmin(fmax(receiver_lat + sample[1] / AIRCRAFT_M_LAT, -90.0), 90.0);
        double sample_lon = receiver_lon + sample[2] / m_per_deg_lon;
        if (sample_lon >= 180.0) sample_lon -= 360.0;
        if (sample_lon < -180.0) sample_lon += 360.0;
        double tangent = (building_tile_elevation(
            environment, inner_elevation, tile_bbox, sample_lat, sample_lon) - receiver_alt_m)
            / sample[0];
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
