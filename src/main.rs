mod core;
mod launcher;
mod platform;

fn main() {
    #[cfg(target_os = "linux")]
    platform::linux::run();
    #[cfg(target_os = "windows")]
    platform::windows::run();
}
