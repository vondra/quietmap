//! Compare device aircraft sums with the CPU.

use super::*;
use popup::aircraft::PASS_CLASS_DB;

/// The card's aircraft sums at the hum lattice and every second block corner against the cores'.
pub(super) fn check_flights(square: &Square, device: usize) -> Result<(), String> {
    let grid = paint::paint::Grid::at_zoom(13).ok_or("no grid at zoom 13")?;
    let corners = paint::paint::corner_points(square, grid)?;
    let boxes = paint::flight_boxes::FlightBoxes::new(square, grid, &corners);
    let requests = paint::flights::lattice_requests(grid, &corners, &boxes);
    let started = Instant::now();
    let cpu: Vec<_> = requests
        .par_iter()
        .map(|request| boxes.sums(square, request))
        .collect::<Result<_, String>>()?;
    let cpu_s = started.elapsed().as_secs_f64();
    let gpu = Gpu::new(device)?;
    let started = Instant::now();
    let device_boxes = paint_gpu::aircraft::DeviceFlightBoxes::upload(&gpu, square, &boxes)?;
    let card = gpu.flight_sums(square, &boxes, &device_boxes, &requests)?;
    let gpu_s = started.elapsed().as_secs_f64();
    // The energy, the passes in progress, the classes' energy over the point's (one where every
    // heard box passes), and the passes' mean level by their energy and by their shares of the
    // time (as energies: a class is `PASS_CLASS_DB` wide, and a pass at a class's edge may fall
    // either side, so the classes are not compared one by one).
    let sums = |sums: &paint::flights::FlightSums, kind: usize| -> [f64; PERIODS] {
        let passes = &sums.passes;
        let mean_level = |weights: &[f64; popup::aircraft::PASS_CLASSES]| {
            let total: f64 = weights.iter().sum();
            let level: f64 = (weights.iter().enumerate())
                .map(|(class, weight)| class as f64 * weight)
                .sum();
            match total > 0.0 {
                true => 10f64.powf(PASS_CLASS_DB * level / total / 10.0),
                false => 0.0,
            }
        };
        match kind {
            0 => sums.energy,
            1 => passes.passing.map(|classes| classes.iter().sum()),
            2 => std::array::from_fn(|p| match sums.energy[p] > 0.0 {
                true => passes.energy[p].iter().sum::<f64>() / sums.energy[p],
                false => 0.0,
            }),
            3 => std::array::from_fn(|p| mean_level(&passes.energy[p])),
            _ => std::array::from_fn(|p| mean_level(&passes.passing[p])),
        }
    };
    for (band, kind) in [0, 1]
        .into_iter()
        .flat_map(|band| (0..5).map(move |kind| (band, kind)))
    {
        let mut differences: Vec<f64> = (cpu.iter().zip(&card))
            .flat_map(|(cpu, card)| {
                (0..PERIODS).filter_map(move |p| {
                    let (c, g): (f64, f64) =
                        (sums(&cpu[band], kind)[p], sums(&card[band], kind)[p]);
                    (c > 0.0 || g > 0.0).then(|| {
                        if c > 0.0 && g > 0.0 {
                            (10.0 * (g / c).log10()).abs()
                        } else {
                            f64::INFINITY
                        }
                    })
                })
            })
            .collect();
        if differences.is_empty() {
            continue;
        }
        differences.sort_unstable_by(f64::total_cmp);
        let at = |q: f64| differences[((differences.len() - 1) as f64 * q) as usize];
        println!(
            "check-flights: band {band} {}: {} sums, |dB| median {:.7}, 99 % {:.6}, 99.9 % {:.6}, worst {:.6}",
            [
                "energy",
                "passes in progress",
                "classes' energy over the energy",
                "passes' mean level by energy",
                "passes' mean level by share",
            ][kind],
            differences.len(),
            at(0.5),
            at(0.99),
            at(0.999),
            differences[differences.len() - 1]
        );
    }
    println!(
        "check-flights: {} requests, cores {cpu_s:.2} s, card {gpu_s:.2} s",
        requests.len()
    );
    Ok(())
}
