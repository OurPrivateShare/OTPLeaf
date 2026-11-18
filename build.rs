fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-light".into())
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/main.slint", config)
        .expect("compile OTPLeaf user interface");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/otpleaf.ico");
        println!("cargo:rerun-if-changed=assets/windows.rc");
        embed_resource::compile_for("assets/windows.rc", ["OTPLeaf"], embed_resource::NONE)
            .manifest_required()
            .expect("embed the OTPLeaf executable icon");
    }
}
