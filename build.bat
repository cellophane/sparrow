set RUSTFLAGS= -C target-cpu=native
set RUSTUP_TOOLCHAIN=nightly
cargo run --release --features=simd,only_final_svg -- -i data/input/temp.json -w 3400 -t 60