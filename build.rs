fn main() {
    // Recompile the UI whenever the .slint sources change. Without this, cargo
    // only reruns this build script when *this* file changes, so edits to the UI
    // would be silently ignored (the window would keep showing the first build).
    println!("cargo:rerun-if-changed=ui/launcher.slint");
    slint_build::compile("ui/launcher.slint").expect("failed to compile Slint UI");

    // Stamp the .exe with our icon so Explorer shows the launcher badge instead
    // of the generic executable one.
    //
    // This is a runtime check rather than `#[cfg(target_os = "windows")]` on
    // purpose: a build script is compiled for the *host*, so the cfg would
    // describe Linux even when cross-compiling to Windows from here. Asking
    // cargo what we are actually targeting is the only correct test.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            // winresource picks the right resource compiler per target: the
            // mingw `windres` when cross-compiling, the SDK `rc.exe` on Windows.
            .compile()
            .expect("failed to embed the Windows application icon");
    }
}
