//! Exact ground-source evaluation at the etalon's sampled outdoor cells.

use super::*;

pub(super) fn run(
    arguments: &[String],
    square: &Square,
    files: &Files,
    weather: &WeatherTable,
) -> Result<(), String> {
    let out = PathBuf::from(option(arguments, "out").ok_or("--out is required")?);
    let sample_step = match number(arguments, "sample-step", 1)? {
        0 => return Err("--sample-step: at least 1".into()),
        step => step as usize,
    };
    let started = Instant::now();
    let gpu = Gpu::new(number(arguments, "device", 0)? as usize)?;
    let device_square = DeviceSquare::upload(&gpu, square, files, weather)?;
    let batch = paint_gpu::batch::GpuBatch::new(&gpu, &device_square, square);
    let grid = paint::paint::Grid::at_zoom(13).ok_or("no grid at zoom 13")?;
    let cells = paint::etalon::etalon(square, grid, Some(&batch), sample_step)?;
    paint::hm3::write(&out, 13, (square.tile.x, square.tile.y), &cells)?;
    println!(
        "etalon {}/{}: {} sources, sample step {sample_step}, {:.1} s on {}",
        square.tile.x,
        square.tile.y,
        square.candidates.len(),
        started.elapsed().as_secs_f64(),
        gpu.name
    );
    Ok(())
}
