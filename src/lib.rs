pub mod app;
pub mod app_state;
pub mod components;
pub mod layout;
pub mod models;
pub mod navigation;
pub mod services;
#[cfg(target_os = "windows")]
mod single_instance;
pub mod theme;
#[cfg(target_os = "windows")]
mod tray;
pub mod views;
