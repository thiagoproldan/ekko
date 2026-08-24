mod config;
mod directory;
mod ekko;
mod item;
mod json;
mod paths;
mod render;
mod storage;

fn main() {
    println!("ekko {}", env!("CARGO_PKG_VERSION"));
}
