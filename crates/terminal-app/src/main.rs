mod action;
mod app;
mod config;
mod layout;
mod session;
mod workspace;

fn main() -> anyhow::Result<()> {
    app::run()
}
