#[cfg(not(target_os = "macos"))]
mod client;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
pub(in crate::top_bar) use client::ApplicationMenu;
#[cfg(target_os = "macos")]
pub(in crate::top_bar) use macos::ApplicationMenu;
