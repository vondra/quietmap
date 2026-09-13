//! Resolve the prepared year's road observations and priors after raw enrichment, continuity and speed taper.

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("usage: roads-finalize PREPARED_YEAR")?;
    let changed = roads_finalize::finalize_year(std::path::Path::new(&path))?;
    println!("{{\"rewritten\":{changed}}}");
    Ok(())
}
