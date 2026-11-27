fn main() {
    println!("cargo:rerun-if-changed=native/macos_capture.m");
    #[cfg(target_os = "macos")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let mut native = cc::Build::new();
        native
            .file("native/macos_capture.m")
            .flag("-fobjc-arc")
            .flag("-std=c11")
            .flag("-mmacosx-version-min=14.2")
            .warnings(true)
            .compile("treelink_macos_capture");

        let runtime = native
            .get_compiler()
            .to_command()
            .arg("-print-file-name=libclang_rt.osx.a")
            .output()
            .expect("query Clang's macOS availability runtime");
        assert!(runtime.status.success(), "Clang runtime query failed");
        let runtime = String::from_utf8(runtime.stdout).expect("Clang runtime path is UTF-8");
        let runtime = std::path::Path::new(runtime.trim());
        assert!(runtime.is_file(), "Clang macOS runtime archive is missing");
        println!(
            "cargo:rustc-link-search=native={}",
            runtime.parent().expect("runtime archive directory").display()
        );
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
        println!("cargo:rustc-link-lib=framework=ScreenCaptureKit");
        println!("cargo:rustc-link-lib=framework=VideoToolbox");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=CoreVideo");
        println!("cargo:rustc-link-lib=framework=CoreImage");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
        println!("cargo:rustc-link-lib=framework=IOKit");
    }
}
