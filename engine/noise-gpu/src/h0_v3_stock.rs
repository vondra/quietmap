//! Fixed current-model CPU field used only for the report-only stock-model delta.

#[path = "h0_v3_sweep.rs"]
mod sweep;

fn main() -> anyhow::Result<()> {
    sweep::run_stock()
}
