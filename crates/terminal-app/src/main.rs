mod action;
mod app;
mod config;
mod layout;
#[cfg(target_os = "macos")]
mod native_menu;
mod palette;
mod session;
mod workspace;

fn main() -> anyhow::Result<()> {
    app::run()
}
