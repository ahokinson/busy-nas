use clap::Parser;

use busy_nas::{
    cli::{Cli, Command},
    config::Config,
    paths::AppPaths,
    process::ProcessRunner,
    service::{ProgramPaths, Service},
    Result,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("busy-nas: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::discover(cli.config);
    let config = Config::load(&paths.config_file)?;
    let mut runner = ProcessRunner::system();
    let programs = ProgramPaths::from_process_runner(&runner);
    let mut service = Service::new(&config, &paths, &mut runner, programs);

    match cli.command {
        Command::Get { project } => service.get(&project),
        Command::Put { project } => service.put(&project),
        Command::Discard { project } => service.discard(&project),
        Command::Status => {
            print!("{}", service.status()?.render());
            Ok(())
        }
        Command::Reclaim { project, force } => service.reclaim(&project, force),
    }
}
