#[cfg(target_os = "linux")]
pub mod elf;
#[cfg(target_os = "macos")]
pub mod macho;
#[cfg(target_os = "windows")]
pub mod win;
pub mod package;
pub mod script;
