//! Compiles the Slint UI and embeds its resources (fonts and icons) in the executable. For a
//! Windows target it also embeds the application manifest (administrator rights, per-monitor
//! DPI awareness), the application icon and the version information in `HomeLink.exe`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config)?;
    embed_windows_resources()?;
    Ok(())
}

/// The manifest, icon and version resource, only for the `HomeLink` binary (not for the test
/// executables, which must start without elevation). `cfg!(windows)` would describe the host
/// of the build, so the target is read from the environment cargo gives the script.
fn embed_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=windows/app.manifest");
    println!("cargo:rerun-if-changed=windows/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_manifest_file("windows/app.manifest")
        .set_icon("windows/app.ico")
        .set("ProductName", "HomeLink")
        .set("FileDescription", "HomeLink")
        .set("OriginalFilename", "HomeLink.exe")
        .set("InternalName", "HomeLink");
    resource.compile_for("HomeLink")?;
    Ok(())
}
