// Receiver terrain/building screening for the airborne CUDA kernels.
#pragma once

__device__ __forceinline__ int terrain_sector(float east_m, float north_m) {
    if (fabsf(east_m) < 1e-9f && fabsf(north_m) < 1e-9f) return 0;
    float angle;
    if (fabsf(east_m) >= fabsf(north_m)) {
        float a = fast_atan(north_m / east_m);
        angle = (east_m >= 0.0f) ? a : a + (float)PI_D;
    } else {
        float a = (float)HALF_PI_D - fast_atan(east_m / north_m);
        angle = (north_m >= 0.0f) ? a : a - (float)PI_D;
    }
    int sector = (int)floorf(angle * (float)TERRAIN_SECTORS / (float)TAU_D);
    sector %= TERRAIN_SECTORS;
    return (sector < 0) ? sector + TERRAIN_SECTORS : sector;
}

__device__ __forceinline__ int building_sector(float east_m, float north_m, int sectors) {
    float angle = atan2f(north_m, east_m);
    if (angle < 0.0f) angle += (float)TAU_D;
    int sector = (int)(angle * (float)sectors / (float)TAU_D);
    return min(sector, sectors - 1);
}

__device__ __forceinline__ float single_edge_diffraction_db(float path_difference_m) {
    float raw = 10.0f * log10f(3.0f + (float)DIFFRACTION_SLOPE_D
        * fmaxf(path_difference_m, 0.0f));
    return fminf(fmaxf(raw - (float)DIFFRACTION_GRAZING_DB_D, 0.0f),
                 (float)DIFFRACTION_CAP_DB_D);
}

__device__ __forceinline__ short packed_tangent(unsigned int packed) {
    return (short)(packed >> 16);
}
__device__ __forceinline__ unsigned short packed_range(unsigned int packed) {
    return (unsigned short)packed;
}
__device__ __forceinline__ float packed_building_tangent(unsigned int packed) {
    return __uint_as_float((packed >> 16) << 16);
}

// Query one entry-major range-max horizon. `entries` stores
// [sector][band][record], transposed so neighbouring receiver threads read
// neighbouring records. Terrain permits an edge at the aircraft foot; vector
// buildings require a strict before-source range, matching their CPU queries.
__device__ __forceinline__ float range_max_screening_db(
    const unsigned int* entries, unsigned long long records, unsigned int record,
    int sector, int bands, float range_scale, float lateral_m, float rel_alt_m,
    bool strict_range, bool building_tangent_encoding)
{
    float source_tangent = rel_alt_m / lateral_m;
    float direct_m = hypotf(lateral_m, rel_alt_m);
    float best_db = 0.0f;
    int first = sector * bands;
    for (int band = 0; band < bands; band++) {
        unsigned int packed = entries[(unsigned long long)(first + band) * records + record];
        unsigned short range_q = packed_range(packed);
        if (range_q == 0) continue;
        float packed_range_m = (float)range_q / range_scale;
        if (strict_range ? (packed_range_m >= lateral_m) : (packed_range_m > lateral_m)) continue;
        float tangent = building_tangent_encoding
            ? packed_building_tangent(packed)
            : (float)packed_tangent(packed) / (float)TAN_SCALE_D;
        if (!(source_tangent < tangent)) continue;
        // The old nearest representation could have been either end of this
        // one-quantum interval. Use the smaller diffraction of both endpoints:
        // it cannot exceed the old value whichever endpoint was selected, while
        // ceiling remains conservative for before-source eligibility. This
        // mirrors the CPU horizon and building queries.
        float edge_db = 1.0e30f;
        for (unsigned int endpoint = 0; endpoint < 2; endpoint++) {
            float range_m = (float)(range_q - 1u + endpoint) / range_scale;
            float edge_z = range_m * tangent;
            float receiver_to_edge_m = hypotf(range_m, edge_z);
            float source_to_edge_m = hypotf(lateral_m - range_m, rel_alt_m - edge_z);
            float delta = fmaxf(receiver_to_edge_m + source_to_edge_m - direct_m, 0.0f);
            edge_db = fminf(edge_db, single_edge_diffraction_db(delta));
        }
        best_db = fmaxf(best_db, edge_db);
    }
    return best_db;
}

__device__ __forceinline__ float terrain_screening_db(
    const unsigned long long* screen, unsigned int record,
    float cpa_east_m, float cpa_north_m, float rel_alt_m, float slant_sq)
{
    unsigned long long records = screen[SCREEN_RECORDS];
    const float* max_sin_sq = reinterpret_cast<const float*>(screen[SCREEN_TERRAIN_MAX_SIN_SQ]);
    if (rel_alt_m > 0.0f && rel_alt_m * rel_alt_m >= slant_sq * max_sin_sq[record]) return 0.0f;
    float lateral_m = hypotf(cpa_east_m, cpa_north_m);
    const unsigned int* entries =
        reinterpret_cast<const unsigned int*>(screen[SCREEN_TERRAIN_ENTRIES]);
    return range_max_screening_db(entries, records, record,
                                  terrain_sector(cpa_east_m, cpa_north_m), TERRAIN_BANDS,
                                  (float)TERRAIN_RANGE_SCALE_D, lateral_m, rel_alt_m, false, false);
}

__device__ __forceinline__ float building_screening_db(
    const unsigned long long* screen, unsigned int record,
    float source_east_m, float source_north_m, float source_rel_alt_m)
{
    const unsigned short* global_max_tangent_bits =
        reinterpret_cast<const unsigned short*>(screen[SCREEN_BUILDING_GLOBAL_MAX_TAN_Q]);
    unsigned short global_max_bits = global_max_tangent_bits[record];
    if (global_max_bits == 0xffffu) return 0.0f;
    float lateral_sq = source_east_m * source_east_m + source_north_m * source_north_m;
    if (lateral_sq <= 1.0f) return 0.0f;
    float global_max_tangent = __uint_as_float((unsigned int)global_max_bits << 16);
    if (source_rel_alt_m >= 0.0f
        && (global_max_tangent <= 0.0f
            || source_rel_alt_m * source_rel_alt_m
                >= lateral_sq * global_max_tangent * global_max_tangent)) return 0.0f;

    unsigned long long records = screen[SCREEN_RECORDS];
    int local_sector = building_sector(source_east_m, source_north_m, BUILDING_LOCAL_SECTORS);
    const unsigned short* local_max_tangent_bits =
        reinterpret_cast<const unsigned short*>(screen[SCREEN_BUILDING_LOCAL_MAX_TAN_Q]);
    unsigned short local_max_bits =
        local_max_tangent_bits[(unsigned long long)local_sector * records + record];
    if (local_max_bits == 0xffffu) return 0.0f;
    float local_max_tangent = __uint_as_float((unsigned int)local_max_bits << 16);
    if (source_rel_alt_m >= 0.0f
        && (local_max_tangent <= 0.0f
            || source_rel_alt_m * source_rel_alt_m
                >= lateral_sq * local_max_tangent * local_max_tangent)) return 0.0f;
    float lateral_m = sqrtf(lateral_sq);
    float source_tangent = source_rel_alt_m / lateral_m;
    if (source_tangent >= local_max_tangent)
        return 0.0f;
    const unsigned int* local_entries =
        reinterpret_cast<const unsigned int*>(screen[SCREEN_BUILDING_LOCAL_ENTRIES]);
    return range_max_screening_db(local_entries, records, record, local_sector,
                                  BUILDING_LOCAL_BANDS, (float)BUILDING_RANGE_SCALE_D,
                                  lateral_m, source_rel_alt_m, true, true);
}

__device__ __forceinline__ float receiver_screening_db(
    const unsigned long long* screen, int pixel,
    float cpa_east_m, float cpa_north_m, float rel_alt_m, float slant_sq,
    float source_east_m, float source_north_m, float source_rel_alt_m)
{
    const unsigned int* record_of_pixel =
        reinterpret_cast<const unsigned int*>(screen[SCREEN_RECORD_OF_PIXEL]);
    unsigned int record = record_of_pixel[pixel];
    if (record == 0xffffffffu) return 0.0f;
    float terrain_db = terrain_screening_db(
        screen, record, cpa_east_m, cpa_north_m, rel_alt_m, slant_sq);
    float building_db = building_screening_db(
        screen, record, source_east_m, source_north_m, source_rel_alt_m);
    return fmaxf(terrain_db, building_db);
}
