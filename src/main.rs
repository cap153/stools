// 在 Windows 上以 GUI 子系统构建，避免双击 exe 时弹出黑色的控制台窗口。
// 该属性在非 Windows 平台会被链接器忽略，不影响 Linux 构建。
#![windows_subsystem = "windows"]

mod core;
mod launcher;
mod platform;

fn main() {
    #[cfg(target_os = "linux")]
    platform::linux::run();
    #[cfg(target_os = "windows")]
    platform::windows::run();
}
