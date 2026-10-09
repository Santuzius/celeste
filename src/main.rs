/// GUI file synchronization client.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {
    /// Open the main window on startup instead of starting hidden in the tray.
    #[arg(long)]
    show: bool,
}

fn main() {
    let cli = <Cli as clap::Parser>::parse();
    celeste::start(cli.show);
}
