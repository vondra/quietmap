//! Upload a parsed square and its fine terrain into the device ABI.

use super::*;

/// Appends a file to a blob at an 8-byte boundary; its offset.
fn append_aligned(blob: &mut Vec<u8>, bytes: &[u8]) -> u64 {
    while !blob.len().is_multiple_of(8) {
        blob.push(0);
    }
    let offset = blob.len() as u64;
    blob.extend_from_slice(bytes);
    offset
}

impl DeviceSquare {
    /// Uploads a square read on the CPU: the ground and obstacles of its rings as stored, its
    /// candidates with their attributes, and the weather table.
    pub fn upload(
        gpu: &Gpu,
        square: &Square,
        files: &Files,
        weather: &WeatherTable,
    ) -> Result<Self, String> {
        let rings = files.rings();
        let index_of: HashMap<TileId, usize> = rings
            .tiles
            .iter()
            .enumerate()
            .map(|(index, &tile)| (tile, index))
            .collect();
        // The ground: popup::scene::Ground of the square, every tile within the ground rings.
        let radius = i64::from(files.ground_rings());
        let side = 2 * radius + 1;
        let n = i64::from(TILES_PER_AXIS);
        let mut terrain_tiles = vec![TerrainTile::default(); (side * side) as usize];
        let frame = square.frame;
        // tiles::terrain::Terrain::sample's node columns per Mercator unit.
        let columns_per_unit = f64::from(360 * NODES_PER_DEGREE) / f64::from(TILES_PER_AXIS);
        let mut nodes = Vec::new();
        // popup::scene::Ground's row table: every lattice row over the scene's tiles, metres north of
        // the origin, the northernmost first.
        let nodes_per_degree = f64::from(NODES_PER_DEGREE);
        let latitude_at = |tile_y: i64| {
            let y = tile_y.clamp(0, n) as f64;
            Mercator { x: 0.0, y }.to_degrees().0
        };
        let north_row =
            (latitude_at(i64::from(square.tile.y) - radius) * nodes_per_degree).ceil() as i64;
        let south_row =
            (latitude_at(i64::from(square.tile.y) + radius + 1) * nodes_per_degree).floor() as i64;
        let scene_rows: Vec<f32> = (south_row..=north_row)
            .rev()
            .map(|row| {
                let y = Mercator::from_degrees(row as f64 / nodes_per_degree, 0.0).y;
                ((frame.origin.y - y) * frame.north_m_per_unit) as f32
            })
            .collect();
        // Each land tile's fine detail, decoded on every core (a tile is megabytes of it).
        let land_tiles: Vec<TileId> = index_of
            .iter()
            .filter(|&(_, &index)| rings.file(index, Kind::Terrain).is_some())
            .map(|(&tile, _)| tile)
            .collect();
        let fine_codes: HashMap<TileId, Vec<i16>> = land_tiles
            .par_iter()
            .filter_map(|&tile| match square.ground.fine_of(tile) {
                Ok(Some(fine)) => Some(
                    fine.codes()
                        .map(|codes| (tile, codes))
                        .map_err(|e| e.to_string()),
                ),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<_, String>>()?;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let y = i64::from(square.tile.y) + dy;
                if !(0..n).contains(&y) {
                    continue;
                }
                let tile = TileId {
                    x: (i64::from(square.tile.x) + dx).rem_euclid(n) as u32,
                    y: y as u32,
                };
                let slot = &mut terrain_tiles[((dy + radius) * side + dx + radius) as usize];
                let Some(&index) = index_of.get(&tile) else {
                    continue;
                };
                match rings.file(index, Kind::Terrain) {
                    None => slot.state = TILE_EMPTY,
                    Some(bytes) => {
                        let terrain = Terrain::parse(bytes).map_err(|e| e.to_string())?;
                        let window = terrain.window();
                        let column_at_origin = window.column_of(frame.origin.x);
                        let nodes_offset = append_aligned(&mut nodes, terrain.node_bytes());
                        let canopy_offset = append_aligned(&mut nodes, terrain.canopy_bytes());
                        let fine_offset = fine_codes
                            .get(&tile)
                            .map_or(0, |codes| append_aligned(&mut nodes, &bytes_of(codes)));
                        *slot = TerrainTile {
                            state: TILE_READ,
                            rows: window.rows,
                            columns: window.columns,
                            row_shift: (north_row - i64::from(window.north_node)) as i32,
                            column_at_origin: column_at_origin as f32,
                            columns_per_metre: (columns_per_unit / frame.east_m_per_unit) as f32,
                            nodes_offset,
                            canopy_offset,
                            fine_offset,
                        };
                    }
                }
            }
        }
        // The obstacles: the scene's own layout, its files as stored.
        let layout = square.obstacles.layout();
        let mut obstacle_tiles = Vec::with_capacity(layout.slots.len());
        let (mut blob, mut cell_max) = (Vec::new(), Vec::<f32>::new());
        for slot in &layout.slots {
            let mut device_tile = ObstacleTile::default();
            match slot {
                None => device_tile.state = TILE_NOT_READ,
                Some(slot) => match slot.cell_max_height_m {
                    None => device_tile.state = TILE_EMPTY,
                    Some(heights) => {
                        let index = index_of
                            .get(&slot.tile)
                            .ok_or("an obstacles tile outside the rings read")?;
                        let bytes = rings
                            .file(*index, Kind::Obstacles)
                            .ok_or("an obstacles tile without its file")?;
                        let obstacles = Obstacles::parse(bytes).map_err(|e| e.to_string())?;
                        let count = |count: usize| count as u32;
                        device_tile = ObstacleTile {
                            state: TILE_READ,
                            outline_count: count(obstacles.outline_count()),
                            vertex_count: count(obstacles.vertex_count()),
                            run_count: count(obstacles.run_count()),
                            occupied_count: count(obstacles.occupied_count()),
                            square_count: count(obstacles.square_count()),
                            offset_x: slot.offset[0] as f32,
                            offset_y: slot.offset[1] as f32,
                            base: append_aligned(&mut blob, bytes),
                            cell_max_offset: cell_max.len() as u64,
                        };
                        cell_max.extend(heights.iter().map(|&height| height as f32));
                    }
                },
            }
            obstacle_tiles.push(device_tile);
        }
        // Candidates and their attributes, the lists flattened.
        let lists = square.attributes.lists();
        let mut first_of_list = Vec::with_capacity(lists.len());
        let mut attributes = Vec::new();
        for list in lists {
            first_of_list.push(attributes.len() as u32);
            attributes.extend(list.iter().map(|source| DeviceAttribute {
                height_m: source.height_m as f32,
                ground_factor: if source.ground_percent == GROUND_FROM_TERRAIN {
                    -1.0
                } else {
                    (f64::from(source.ground_percent) / 100.0) as f32
                },
                platform_half_width_m: source.platform_half_width_m as f32,
                exclusion_radius_m: source.exclusion_radius_m as f32,
                footprint_id: source.footprint_id,
                energy: source.energy.map(|period| period.map(|value| value as f32)),
            }));
        }
        let candidates: Vec<DeviceCandidate> = square
            .candidates
            .par_iter()
            .map(|candidate| {
                let [a, b] = candidate.ends_m;
                let ground_m = candidate.traced_ground_m(&square.ground)?;
                Ok(DeviceCandidate {
                    ax: a[0] as f32,
                    ay: a[1] as f32,
                    bx: b[0] as f32,
                    by: b[1] as f32,
                    ground0: ground_m[0] as f32,
                    ground1: ground_m[1] as f32,
                    line: u32::from(candidate.line),
                    attribute: first_of_list[candidate.attribute.list as usize]
                        + candidate.attribute.index,
                    on_deck: u32::from(candidate.deck_m.is_some()),
                    layer: candidate.layer as u32,
                })
            })
            .collect::<Result<_, String>>()?;
        let device = &gpu.device;
        let upload = |bytes: Vec<u8>| driver(device.htod_sync_copy(&bytes));
        let pointer = |slice: &CudaSlice<u8>| *slice.device_ptr();
        // An empty buffer still needs an allocation to point at.
        let nonempty = |mut bytes: Vec<u8>| {
            if bytes.is_empty() {
                bytes.push(0);
            }
            bytes
        };
        let has_fine = terrain_tiles.iter().any(|tile| tile.fine_offset != 0);
        let terrain_tiles = upload(bytes_of(&terrain_tiles))?;
        let nodes = upload(nonempty(nodes))?;
        let scene_row_count = scene_rows.len() as u32;
        let scene_rows = driver(device.htod_sync_copy(&scene_rows))?;
        let centre_west_node = Window::of_tile(square.tile).west_node;
        let obstacle_tiles = upload(nonempty(bytes_of(&obstacle_tiles)))?;
        let blob = upload(nonempty(blob))?;
        let cell_max = driver(device.htod_sync_copy(&if cell_max.is_empty() {
            vec![0.0]
        } else {
            cell_max
        }))?;
        let weather_nodes = upload(weather.node_bytes().to_vec())?;
        let candidates_bytes = upload(nonempty(bytes_of(&candidates)))?;
        let attributes_bytes = upload(nonempty(bytes_of(&attributes)))?;
        let descriptor = SquareScene {
            ground: TerrainScene {
                tile_x_at_origin: (frame.origin.x - f64::from(square.tile.x)) as f32,
                tile_y_at_origin: (frame.origin.y - f64::from(square.tile.y)) as f32,
                tiles_per_metre_east: (1.0 / frame.east_m_per_unit) as f32,
                tiles_per_metre_north: (1.0 / frame.north_m_per_unit) as f32,
                radius,
                tiles: pointer(&terrain_tiles),
                nodes: pointer(&nodes),
                scene_rows: *scene_rows.device_ptr(),
                scene_row_count,
                column_at_origin: (frame.origin.x * columns_per_unit
                    - f64::from(180 * NODES_PER_DEGREE)
                    - f64::from(centre_west_node)) as f32,
                columns_per_metre: (columns_per_unit / frame.east_m_per_unit) as f32,
                has_fine: u32::from(has_fine),
                equator_row: if (south_row..=north_row).contains(&0) {
                    north_row
                } else {
                    -1
                },
            },
            obstacles: ObstacleScene {
                origin_x: layout.origin[0] as f32,
                origin_y: layout.origin[1] as f32,
                steps_per_metre_x: (1.0 / layout.metres_per_step[0]) as f32,
                steps_per_metre_y: (1.0 / layout.metres_per_step[1]) as f32,
                metres_per_step_x: layout.metres_per_step[0] as f32,
                metres_per_step_y: layout.metres_per_step[1] as f32,
                centre_x: layout.centre.x,
                centre_y: layout.centre.y,
                origin_cell_in_centre_x: layout.origin_cell_in_centre[0],
                origin_cell_in_centre_y: layout.origin_cell_in_centre[1],
                radius: layout.radius,
                tiles: pointer(&obstacle_tiles),
                blob: pointer(&blob),
                cell_max_height: *cell_max.device_ptr(),
            },
            weather_nodes: pointer(&weather_nodes),
            candidates: pointer(&candidates_bytes),
            attributes: pointer(&attributes_bytes),
            frame_origin_x: frame.origin.x as f32,
            frame_origin_y: frame.origin.y as f32,
            frame_east_m_per_unit: frame.east_m_per_unit as f32,
            frame_north_m_per_unit: frame.north_m_per_unit as f32,
        };
        let scene = upload(bytes_of(&[descriptor]))?;
        let buffers = vec![
            terrain_tiles,
            nodes,
            obstacle_tiles,
            blob,
            weather_nodes,
            candidates_bytes,
            attributes_bytes,
        ];
        let bytes = buffers.iter().map(|b| b.len() as u64).sum::<u64>()
            + 4 * (scene_rows.len() + cell_max.len()) as u64;
        Ok(DeviceSquare {
            scene,
            candidates: square.candidates.len() as u32,
            _buffers: buffers,
            _floats: vec![cell_max, scene_rows],
            bytes,
        })
    }
}
