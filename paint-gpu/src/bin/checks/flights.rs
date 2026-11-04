//! Compare device aircraft sums with the CPU.

use super::*;

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
    for (band, kind) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let mut differences: Vec<f64> = (cpu.iter().zip(&card))
            .flat_map(|(cpu, card)| {
                (0..PERIODS).filter_map(move |p| {
                    let (c, g) = (
                        [cpu[band].energy, cpu[band].energy_times_lambda][kind][p],
                        [card[band].energy, card[band].energy_times_lambda][kind][p],
                    );
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
            ["energy", "energy x lambda"][kind],
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
