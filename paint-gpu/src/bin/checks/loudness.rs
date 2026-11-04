//! Compare device loudness with CPU distributions.

use super::*;

/// The card's loudness of `count` random outdoor pixels of the square against the cores'.
pub(super) fn check_loudness(
    square: &Square,
    count: usize,
    seed: u64,
    device: usize,
) -> Result<(), String> {
    let grid = paint::paint::Grid::at_zoom(13).ok_or("no grid at zoom 13")?;
    let started = Instant::now();
    let sounds = paint::etalon::sounds_at(square, grid, count, seed)?;
    let sounds_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let cpu: Vec<[f64; LAYERS + 1]> = (sounds.par_iter())
        .map(|sound| sound.ndens(popup::percentiles::Resolution::MAP))
        .collect();
    let cpu_s = started.elapsed().as_secs_f64();
    let (mut input, mut offsets) = (Vec::new(), Vec::new());
    for sound in &sounds {
        offsets.push(input.len() as u64);
        sound.pack(
            popup::percentiles::Resolution::MAP.weather_states,
            &mut input,
        );
    }
    let gpu = Gpu::new(device)?;
    let started = Instant::now();
    let card = gpu.pixel_loudness(&input, &offsets)?;
    let gpu_s = started.elapsed().as_secs_f64();
    let phon = |sone: f64| {
        if sone > 0.0 {
            40.0 + 10.0 * sone.log2()
        } else {
            f64::NEG_INFINITY
        }
    };
    let names = [
        "road",
        "railway",
        "industrial",
        "building",
        "ship",
        "aircraft",
        "total",
    ];
    for (k, name) in names.iter().enumerate() {
        let mut differences: Vec<(f64, usize)> = (0..sounds.len())
            .filter_map(|pixel| {
                let (c, g) = (cpu[pixel][k], f64::from(card[pixel][k]));
                // Under 1/16 sone (0 phon) both count as silence.
                if c < 1.0 / 16.0 && g < 1.0 / 16.0 {
                    return None;
                }
                Some(((phon(g.max(1e-6)) - phon(c.max(1e-6))).abs(), pixel))
            })
            .collect();
        differences.sort_by(|a, b| a.0.total_cmp(&b.0));
        let at = |q: f64| {
            differences
                .get(((differences.len() as f64 - 1.0) * q).round() as usize)
                .map_or(0.0, |d| d.0)
        };
        let over = |limit: f64| differences.iter().filter(|d| d.0 > limit).count();
        let worst = differences.last().copied().unwrap_or((0.0, 0));
        println!(
            "{name:10} {} pixels: |card - cores| phon median {:.3}, p99 {:.3}, max {:.3} (pixel {}: cores {:.4} card {:.4} sone); over 0.1 {}, 0.5 {}, 1 {}",
            differences.len(),
            at(0.5),
            at(0.99),
            worst.0,
            worst.1,
            cpu[worst.1][k],
            card[worst.1][k],
            over(0.1),
            over(0.5),
            over(1.0)
        );
    }
    println!(
        "{count} pixels: sounds {sounds_s:.1} s, cores {cpu_s:.2} s on {} threads, card {gpu_s:.3} s on {} ({} floats)",
        rayon::current_num_threads(),
        gpu.name,
        input.len()
    );
    Ok(())
}
