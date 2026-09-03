fn main() {
    // Recompile the UI whenever the .slint sources change. Without this, cargo
    // only reruns this build script when *this* file changes, so edits to the UI
    // would be silently ignored (the window would keep showing the first build).
    println!("cargo:rerun-if-changed=ui/launcher.slint");
    slint_build::compile("ui/launcher.slint").expect("failed to compile Slint UI");
}
