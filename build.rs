fn main() {
    println!("cargo:rerun-if-env-changed=RESONANCE_DEFAULT_API_URL");
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let api_url = std::env::var("RESONANCE_DEFAULT_API_URL")
            .expect("Set RESONANCE_DEFAULT_API_URL when building a release.");
        if !api_url.starts_with("https://") {
            panic!("RESONANCE_DEFAULT_API_URL must use HTTPS in release builds.");
        }
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/icons/resonance.ico");
        resource
            .compile()
            .expect("Failed to embed the Resonance application icon");
    }
}
