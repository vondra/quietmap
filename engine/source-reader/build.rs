//! Build script for the source-reader NAPI addon — runs `napi_build::setup()`
//! so the Node `.node` binding (popup engine) links correctly.
extern crate napi_build;
fn main() {
    napi_build::setup();
}
