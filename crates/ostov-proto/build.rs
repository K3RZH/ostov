//! `.proto` → Rust через protox: ставить `protoc` никому в команде не нужно.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let files = protox::compile(["ostov/v1/engine.proto"], ["proto"])?;
    tonic_prost_build::configure().compile_fds(files)?;
    Ok(())
}
