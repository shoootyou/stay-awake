fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=CoreLocation");
        println!("cargo:rustc-link-lib=framework=CoreWLAN");

        cc::Build::new()
            .file("src/corewlan_bridge.m")
            .flag("-fobjc-arc")
            .flag("-mmacosx-version-min=11.0")
            .compile("corewlan_bridge");
    }

    tauri_build::build()
}
