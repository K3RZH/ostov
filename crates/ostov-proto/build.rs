//! `.proto` → Rust через protox: ставить `protoc` никому в команде не нужно.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let files = protox::compile(["ostov/v1/engine.proto"], ["proto"])?;
    // Debug у сообщений с паролем пишется руками (lib.rs): пароль не должен попасть в лог.
    tonic_prost_build::configure()
        .skip_debug([
            ".ostov.v1.Credentials",
            ".ostov.v1.NewUser",
            ".ostov.v1.UserPassword",
        ])
        .compile_fds(files)?;
    Ok(())
}
