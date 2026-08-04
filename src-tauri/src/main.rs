// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Use mimalloc as the global allocator. With the `override` feature it also
// replaces the system malloc via symbol interposition, so C libraries (notably
// ONNX Runtime / ort, candle) allocate through mimalloc too.
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    mcphub_lib::run();
}
