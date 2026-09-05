use std::env;
use std::error::Error;
use std::path::PathBuf;
use swayipc::Connection as SwayConnection;
use swaymnesia::config::Config;
use swaymnesia::session::Session;
use swaymnesia::{restore, session};

const USAGE: &str = "usage: swaymnesia <save|restore|watch|dump> [path]

  save     write the current sway session to 'path'
  restore  start the programs recorded in 'path' and lay their windows out
  watch    rewrite 'path' whenever the session changes
  dump     print the session stored in 'path' as text";

fn main() {
    if let Err(e) = run() {
        eprintln!("swaymnesia: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        println!("{USAGE}");
        return Ok(());
    };

    let path = args
        .next()
        .map_or_else(Session::default_path, PathBuf::from);

    match command.as_str() {
        "save" => swaymnesia::save_session(
            &session::capture(&mut SwayConnection::new()?, &Config::load()?)?,
            &path,
        )?,
        "restore" => {
            let session = swaymnesia::load_session(&path)?;
            restore::restore(&session)?
        }
        "watch" => swaymnesia::watch(&path)?,
        "dump" => {
            let session = swaymnesia::load_session(&path)?;
            print!("{}", session.dump())
        }
        "version" | "--version" => println!("swaymnesia {}", std::env!("CARGO_PKG_VERSION")),
        _ => println!("{USAGE}"),
    }
    Ok(())
}
