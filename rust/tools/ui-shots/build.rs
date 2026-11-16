//! Compiles a copy of the app's UI (`ui/`, made by run.sh) with the scroll position of the
//! wizard exposed, so that the end of each step can be rendered.
fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("the UI compiles");
}
