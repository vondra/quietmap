//! Random source/receiver pairs compared directly between the card and CPU.

use super::*;

pub(super) fn check(
    arguments: &[String],
    square: &Square,
    files: &Files,
    weather: &WeatherTable,
    device: usize,
    (pairs, seed): (usize, u64),
) -> Result<(), String> {
    let mut random = SplitMix(seed);
    let parse = |name, default| number(arguments, name, default);
    let started = Instant::now();
    let gpu = Gpu::new(device)?;
    let device_square = DeviceSquare::upload(&gpu, square, files, weather)?;
    eprintln!(
        "{}: kernels compiled, {:.0} MB uploaded in {:.1} s",
        gpu.name,
        device_square.bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    // The pairs: outdoor receivers anywhere in the square, sources near or anywhere in reach.
    let corner = |dx: f64, dy: f64| paint::paint::metres_at(square, 1, dx, dy);
    let (a, b) = (corner(0.0, 0.0), corner(1.0, 1.0));
    let mut receivers = Vec::with_capacity(pairs);
    let mut chosen = Vec::with_capacity(pairs);
    let mut near = Vec::new();
    // With --group G, G neighbouring receivers (rows of 8, 6 m apart, z13's pixels) share one
    // source, as a warp of the painter would take them.
    let group = parse("group", 1)? as usize;
    let near_m = parse("near", 300)? as f64;
    let reach_m = parse("reach", GROUND_REACH_M as u64)? as f64;
    // --kind line, point or deck (a bridge's pieces): candidates of that kind only.
    let kind = option(arguments, "kind");
    let of_kind = |line: bool, on_deck: bool| match kind {
        Some("line") => line,
        Some("point") => !line,
        Some("deck") => on_deck,
        _ => true,
    };
    let mut base = [0.0, 0.0];
    while receivers.len() < pairs {
        let member = receivers.len() % group;
        if member == 0 {
            base = [
                a[0] + random.unit() * (b[0] - a[0]),
                a[1] + random.unit() * (b[1] - a[1]),
            ];
        }
        let position = [
            base[0] + 6.0 * (member % 8) as f64,
            base[1] - 6.0 * (member / 8) as f64,
        ];
        let indoor = square.obstacles.enclosing_building_id(position)?.is_some();
        if member > 0 {
            // A group stays whole: an indoor member stands at the group's first receiver.
            let first = receivers.len() - member;
            receivers.push(if indoor { receivers[first] } else { position });
            chosen.push(chosen[first]);
            continue;
        }
        if indoor {
            continue;
        }
        let index = if (receivers.len() / group).is_multiple_of(2) {
            square.index.within(position, position, near_m, &mut near);
            near.retain(|&index| {
                let candidate = &square.candidates[index as usize];
                candidate.distance_from(position) <= near_m
                    && of_kind(candidate.line, candidate.deck_m.is_some())
            });
            if near.is_empty() {
                continue;
            }
            near[(random.next_u64() % near.len() as u64) as usize]
        } else {
            let index = (random.next_u64() % square.candidates.len() as u64) as u32;
            let candidate = &square.candidates[index as usize];
            if candidate.distance_from(position) > reach_m
                || !of_kind(candidate.line, candidate.deck_m.is_some())
            {
                continue;
            }
            index
        };
        receivers.push(position);
        chosen.push(index);
    }
    // Launches of a quarter of a million pairs, each pair its own point (its bands apart, 150 MB);
    // a first launch brings the card to its clocks.
    const LAUNCH: usize = 1 << 18;
    let own = vec![0u64; LAUNCH];
    let band_weights = vec![1.0f32; LAUNCH];
    let point_of_pair: Vec<u32> = (0..LAUNCH as u32).collect();
    let mut bands = gpu.bands(LAUNCH)?;
    let warm = pairs.min(20_000);
    gpu.evaluate_pairs(
        &device_square,
        (&receivers[..warm], &own[..warm]),
        (
            &chosen[..warm],
            &point_of_pair[..warm],
            &point_of_pair[..warm],
            &band_weights[..warm],
        ),
        &mut bands,
        None,
    )?;
    let (mut gpu_sums, mut failed, mut gpu_bands) = (Vec::new(), Vec::new(), Vec::new());
    let mut gpu_s = 0.0;
    for start in (0..pairs).step_by(LAUNCH) {
        let count = LAUNCH.min(pairs - start);
        bands = gpu.bands(LAUNCH)?;
        let started = Instant::now();
        let (sums, failures) = gpu.evaluate_pairs(
            &device_square,
            (&receivers[start..start + count], &own[..count]),
            (
                &chosen[start..start + count],
                &point_of_pair[..count],
                &point_of_pair[..count],
                &band_weights[..count],
            ),
            &mut bands,
            None,
        )?;
        gpu_s += started.elapsed().as_secs_f64();
        gpu_sums.extend(sums);
        failed.extend(failures);
        gpu_bands.extend_from_slice(&gpu.download(&bands)?[..count * LAYERS * PERIODS * BANDS]);
    }
    let started = Instant::now();
    let cpu: Vec<Result<(PairEnergy, Bands), String>> = receivers
        .par_iter()
        .zip(&chosen)
        .map_init(Scratch::default, |scratch, (&position, &index)| {
            // This compares ray energy before reflection. Grouped receivers can extend beyond the
            // square's prepared reflection window; their ground and weather still belong to its scene.
            whole_energy(
                square,
                &Point::with_receiver_properties(square, position, 0, 0.0)?,
                index,
                scratch,
            )
        })
        .collect();
    let cpu_s = started.elapsed().as_secs_f64();
    // Agreement in dB where both answered: each pair's Lden energy mixed and in either state, and
    // each band carrying a thousandth of its period's energy or more.
    let (mut mixed, mut calm, mut downwind, mut spectrum) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut gpu_failed, mut cpu_failed, mut silent) = (0, 0, 0);
    let decibels = |g: f64, c: f64| {
        if g > 0.0 && c > 0.0 {
            Some((10.0 * (g / c).log10()).abs())
        } else if g == 0.0 && c == 0.0 {
            None
        } else {
            Some(f64::INFINITY)
        }
    };
    for pair in 0..pairs {
        if failed[pair] != 0 {
            gpu_failed += 1;
            continue;
        }
        let Ok((energy, cpu_bands)) = &cpu[pair] else {
            cpu_failed += 1;
            continue;
        };
        let card = |k: usize| std::array::from_fn(|p| f64::from(gpu_sums[pair][k * PERIODS + p]));
        let Some(difference) = decibels(lden_energy(&card(0)), lden_energy(&energy.mixed)) else {
            silent += 1;
            continue;
        };
        mixed.push((difference, pair));
        calm.extend(decibels(
            lden_energy(&card(1)),
            lden_energy(&energy.states[0]),
        ));
        downwind.extend(decibels(
            lden_energy(&card(2)),
            lden_energy(&energy.states[1]),
        ));
        let layer = square.candidates[chosen[pair] as usize].layer as usize;
        for (period, cpu_period) in cpu_bands.iter().enumerate() {
            for (band, &value) in cpu_period.iter().enumerate() {
                if value >= 1e-3 * energy.mixed[period] {
                    let at = ((pair * LAYERS + layer) * PERIODS + period) * BANDS + band;
                    spectrum.extend(decibels(f64::from(gpu_bands[at]), value));
                }
            }
        }
    }
    let count = |code: u32| failed.iter().filter(|&&c| c & code != 0).count();
    println!(
        "pairs {pairs}: compared {}, silent {silent}, GPU failed {gpu_failed} (not read {}, no terrain {}, capacity {}, line cutoff {}), CPU failed {cpu_failed}",
        mixed.len(),
        count(paint_gpu::device::FAILED_NOT_READ),
        count(paint_gpu::device::FAILED_NO_TERRAIN),
        count(paint_gpu::device::FAILED_CAPACITY),
        count(paint_gpu::device::FAILED_LINE_CUTOFF),
    );
    mixed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut lden: Vec<f64> = mixed.iter().map(|d| d.0).collect();
    for (name, values) in [
        ("Lden", &mut lden),
        ("calm Lden", &mut calm),
        ("downwind Lden", &mut downwind),
        ("band", &mut spectrum),
    ] {
        values.sort_by(f64::total_cmp);
        let at = |q: f64| {
            values
                .get(((values.len() as f64 - 1.0) * q).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        println!(
            "|GPU - CPU| {name} dB: median {:.2e}, p99 {:.2e}, p99.9 {:.2e}, p99.99 {:.2e}, max {:.2e}; over 1 {}, over 3 {}",
            at(0.5),
            at(0.99),
            at(0.999),
            at(0.9999),
            at(1.0),
            values.iter().filter(|&&value| value > 1.0).count(),
            values.iter().filter(|&&value| value > 3.0).count()
        );
    }
    println!(
        "time: GPU {gpu_s:.3} s ({:.0} ns a pair), CPU {cpu_s:.3} s on {} threads ({:.0} ns a pair)",
        gpu_s / pairs as f64 * 1e9,
        rayon::current_num_threads(),
        cpu_s / pairs as f64 * 1e9
    );
    for &(difference, pair) in mixed.iter().take(8) {
        let candidate = &square.candidates[chosen[pair] as usize];
        println!(
            "  worst {difference:.3e} dB: receiver {:.1},{:.1} candidate {} ({}, {:.0} m) GPU {:?} CPU {:?}",
            receivers[pair][0],
            receivers[pair][1],
            chosen[pair],
            if candidate.line { "line" } else { "point" },
            candidate.distance_from(receivers[pair]),
            &gpu_sums[pair][..PERIODS],
            cpu[pair].as_ref().ok().map(|(energy, _)| energy.mixed)
        );
    }
    for pair in (0..pairs).filter(|&p| failed[p] != 0).take(5) {
        println!(
            "  GPU failure {} at pair {pair}: CPU {:?}",
            failed[pair],
            cpu[pair]
                .as_ref()
                .map(|(energy, _)| lden_energy(&energy.mixed))
        );
    }
    Ok(())
}
