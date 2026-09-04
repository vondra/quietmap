#!/usr/bin/env bash
# Compute gdalwarp -te extent for node-registered grids.
#
# Node-registered: pixel CENTERS at integer degrees (like SRTM).
# Pixel 0 center = origin, pixel N-1 center = origin + 1°.
# gdalwarp -te needs pixel EDGES → expand by half-pixel.
#
# Usage: node_extent LON LAT N
# Prints: xmin ymin xmax ymax

node_extent() {
    local lon=$1 lat=$2 n=$3
    python3 -c "hp=0.5/($n-1); print(f'{$lon-hp:.10f} {$lat-hp:.10f} {$lon+1+hp:.10f} {$lat+1+hp:.10f}')"
}
export -f node_extent
