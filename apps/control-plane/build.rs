use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROTOC").is_none() {
        let system_protoc = Command::new("protoc")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);

        if !system_protoc {
            std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
        }
    }

    tonic_build::configure().compile_protos(&["../../proto/execution.proto"], &["../../proto"])?;
    println!("cargo:rerun-if-changed=../../proto/execution.proto");
    Ok(())
}
