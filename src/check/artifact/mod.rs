#[cfg(target_os = "linux")]
pub mod elf;
#[cfg(target_os = "macos")]
pub mod macho;
pub mod package;
pub mod script;
