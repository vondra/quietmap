//! CUDA entry points for shared-corner triage and exact retained-source pixel painting.

#include "relevant_source_pair.cuh"

#include <stdio.h>

__global__ void evaluate_corner_source_pairs_kernel(
    DeviceScenePointers scene,
    const uint32_t* corner_offsets,
    const uint32_t* corner_source_indices,
    const float* corner_x_m,
    const float* corner_y_m,
    const float* corner_reflection_db,
    float* pair_period_energy
) {
    const uint32_t corner = blockIdx.x;
    const float receiver_x = corner_x_m[corner];
    const float receiver_y = corner_y_m[corner];
    const float receiver_altitude = sample_scene_raster(scene, receiver_x, receiver_y).elevation_m
                                    + QUIETMAP_DEFAULT_RECEIVER_HEIGHT_M;
    for (uint32_t pair = corner_offsets[corner] + threadIdx.x;
         pair < corner_offsets[corner + 1];
         pair += blockDim.x) {
        float energy[QUIETMAP_PERIOD_COUNT] = {};
        evaluate_source_receiver_energy(
            scene, corner_source_indices[pair], receiver_x, receiver_y,
            receiver_altitude, corner_reflection_db[corner], energy);
        for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
            pair_period_energy[pair * QUIETMAP_PERIOD_COUNT + period] = energy[period];
        }
    }
}

__global__ void paint_relevant_sources_kernel(
    DeviceScenePointers scene,
    const uint32_t* block_offsets,
    const uint32_t* relevant_source_indices,
    const float* background_energy,
    const float* receiver_x_m,
    const float* receiver_y_m,
    const float* receiver_altitude_m,
    const float* receiver_reflection_db,
    float* output_period_energy
) {
    const uint32_t pixel = blockIdx.x * blockDim.x + threadIdx.x;
    if (pixel >= QUIETMAP_TILE_PIXEL_SIDE * QUIETMAP_TILE_PIXEL_SIDE) {
        return;
    }
    const uint32_t row = pixel / QUIETMAP_TILE_PIXEL_SIDE;
    const uint32_t column = pixel - row * QUIETMAP_TILE_PIXEL_SIDE;
    const uint32_t block = (row / QUIETMAP_BLOCK_PIXEL_SIDE) * QUIETMAP_BLOCKS_PER_TILE_SIDE
                           + column / QUIETMAP_BLOCK_PIXEL_SIDE;
    const float block_x = (static_cast<float>(column % QUIETMAP_BLOCK_PIXEL_SIDE) + 0.5f)
                          / QUIETMAP_BLOCK_PIXEL_SIDE;
    const float block_y = (static_cast<float>(row % QUIETMAP_BLOCK_PIXEL_SIDE) + 0.5f)
                          / QUIETMAP_BLOCK_PIXEL_SIDE;
    float accumulated[QUIETMAP_PERIOD_COUNT];
    for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
        const uint32_t first = (block * 4) * QUIETMAP_PERIOD_COUNT + period;
        const float top = fmaf(
            block_x,
            background_energy[first + QUIETMAP_PERIOD_COUNT] - background_energy[first],
            background_energy[first]);
        const float bottom = fmaf(
            block_x,
            background_energy[first + 3 * QUIETMAP_PERIOD_COUNT]
                - background_energy[first + 2 * QUIETMAP_PERIOD_COUNT],
            background_energy[first + 2 * QUIETMAP_PERIOD_COUNT]);
        accumulated[period] = fmaf(block_y, bottom - top, top);
    }
    for (uint32_t position = block_offsets[block]; position < block_offsets[block + 1]; ++position) {
        float source_energy[QUIETMAP_PERIOD_COUNT] = {};
        if (!evaluate_source_receiver_energy(
                scene, relevant_source_indices[position], receiver_x_m[column], receiver_y_m[row],
                receiver_altitude_m[pixel], receiver_reflection_db[pixel], source_energy)) {
            continue;
        }
        for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
            accumulated[period] += source_energy[period];
        }
    }
    for (int period = 0; period < QUIETMAP_PERIOD_COUNT; ++period) {
        output_period_energy[pixel * QUIETMAP_PERIOD_COUNT + period] = accumulated[period];
    }
}

extern "C" const char* relevant_source_cuda_error_string(int status) {
    return cudaGetErrorString(static_cast<cudaError_t>(status));
}

extern "C" int relevant_source_cuda_initialize() {
    int device_count = 0;
    cudaError_t status = cudaGetDeviceCount(&device_count);
    if (status != cudaSuccess) {
        return status;
    }
    if (device_count < 1) {
        return cudaErrorNoDevice;
    }
    return cudaSetDevice(0);
}

extern "C" int relevant_source_cuda_allocate(void** pointer, size_t bytes) {
    return cudaMalloc(pointer, bytes > 0 ? bytes : 1);
}

extern "C" int relevant_source_cuda_free(void* pointer) {
    return pointer == nullptr ? cudaSuccess : cudaFree(pointer);
}

extern "C" int relevant_source_cuda_copy_to_device(void* destination, const void* source, size_t bytes) {
    return bytes == 0 ? cudaSuccess
                      : cudaMemcpy(destination, source, bytes, cudaMemcpyHostToDevice);
}

extern "C" int relevant_source_cuda_copy_to_host(void* destination, const void* source, size_t bytes) {
    return bytes == 0 ? cudaSuccess
                      : cudaMemcpy(destination, source, bytes, cudaMemcpyDeviceToHost);
}

template <typename Launch>
int timed_cuda_launch(Launch launch, float* elapsed_milliseconds) {
    cudaEvent_t started;
    cudaEvent_t finished;
    cudaError_t status = cudaEventCreate(&started);
    if (status != cudaSuccess) {
        return status;
    }
    status = cudaEventCreate(&finished);
    if (status == cudaSuccess) {
        status = cudaEventRecord(started);
    }
    if (status == cudaSuccess) {
        launch();
        status = cudaGetLastError();
    }
    if (status == cudaSuccess) {
        status = cudaEventRecord(finished);
    }
    if (status == cudaSuccess) {
        status = cudaEventSynchronize(finished);
    }
    if (status == cudaSuccess) {
        status = cudaEventElapsedTime(elapsed_milliseconds, started, finished);
    }
    cudaEventDestroy(started);
    cudaEventDestroy(finished);
    return status;
}

extern "C" int relevant_source_cuda_evaluate_corners(
    const DeviceScenePointers* scene,
    const uint32_t* corner_offsets,
    const uint32_t* corner_source_indices,
    const float* corner_x_m,
    const float* corner_y_m,
    const float* corner_reflection_db,
    float* pair_period_energy,
    float* elapsed_milliseconds
) {
    return timed_cuda_launch([&] {
        evaluate_corner_source_pairs_kernel<<<QUIETMAP_CORNER_COUNT, 256>>>(
            *scene, corner_offsets, corner_source_indices, corner_x_m, corner_y_m,
            corner_reflection_db, pair_period_energy);
    }, elapsed_milliseconds);
}

extern "C" int relevant_source_cuda_paint_tile(
    const DeviceScenePointers* scene,
    const uint32_t* block_offsets,
    const uint32_t* relevant_source_indices,
    const float* background_energy,
    const float* receiver_x_m,
    const float* receiver_y_m,
    const float* receiver_altitude_m,
    const float* receiver_reflection_db,
    float* output_period_energy,
    float* elapsed_milliseconds
) {
    constexpr uint32_t threads = 256;
    constexpr uint32_t pixels = QUIETMAP_TILE_PIXEL_SIDE * QUIETMAP_TILE_PIXEL_SIDE;
    return timed_cuda_launch([&] {
        paint_relevant_sources_kernel<<<(pixels + threads - 1) / threads, threads>>>(
            *scene, block_offsets, relevant_source_indices, background_energy,
            receiver_x_m, receiver_y_m, receiver_altitude_m, receiver_reflection_db,
            output_period_energy);
    }, elapsed_milliseconds);
}
