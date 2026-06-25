fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/audio-orbit.ico");
    resource.set_manifest(include_str!("src/audio-orbit.exe.manifest"));

    if let Err(error) = resource.compile() {
        panic!("failed to compile Windows resources: {error}");
    }
}
