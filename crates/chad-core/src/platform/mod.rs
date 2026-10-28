//! OS integration behind one interface per concern; the impl is picked by target.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::SystemSecrets;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::SystemSecrets;

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
pub use unsupported::SystemSecrets;
