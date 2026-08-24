mod config;
mod directory;
mod item;
mod json;
mod paths;
mod render;
mod storage;

fn main() {
    println!("ekko {}", env!("CARGO_PKG_VERSION"));
}
