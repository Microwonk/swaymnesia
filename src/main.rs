mod format;
mod restore;
mod session;

use crate::session::Workspace;
use crate::session::{Session, Window};
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use swayipc::{Connection as SwayConnection, Event, EventType, WindowChange};

const USAGE: &str = "usage: swaymnesia <save|restore|watch|dump> [path]

  save     write the current sway session to path
  restore  start the programs recorded in path and lay their windows out
  watch    rewrite path whenever the session changes
  dump     print the session stored in path as text";

/// How long the session has to stay unchanged before `watch` writes it to the session file
const SETTLE: Duration = Duration::from_secs(1);

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

    let path = args.next().map_or_else(default_path, PathBuf::from);

    match command.as_str() {
        "save" => save(&session::capture(&mut SwayConnection::new()?)?, &path)?,
        "restore" => restore::restore(&load(&path)?)?,
        "watch" => watch(&path)?,
        "dump" => print!("{}", dump(&load(&path)?)),
        _ => println!("{USAGE}"),
    }
    Ok(())
}

fn default_path() -> PathBuf {
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data_home.join("swaymnesia/session.swmn")
}

/// Writes through a temporary file for an atomic write operation
fn save(session: &Session, path: &Path) -> Result<(), Box<dyn Error>> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, format::encode(session))?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn load(path: &Path) -> Result<Session, Box<dyn Error>> {
    let context = |e: &dyn fmt::Display| format!("{}: {e}", path.display());
    let bytes = fs::read(path).map_err(|e| context(&e))?;
    format::decode(&bytes).map_err(|e| context(&e).into())
}

/// Watches the session for as long as sway runs, saving when settled
fn watch(path: &Path) -> Result<(), Box<dyn Error>> {
    let events = SwayConnection::new()?.subscribe([EventType::Window, EventType::Workspace])?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for event in events.flatten() {
            if changes_session(&event) && tx.send(()).is_err() {
                return;
            }
        }
    });

    let mut conn = SwayConnection::new()?;
    let mut changed_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(SETTLE) {
            Ok(()) => {
                changed_at.get_or_insert_with(Instant::now);
            }
            Err(RecvTimeoutError::Timeout) => {}
            // sway has exited
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if changed_at.is_some_and(|at| at.elapsed() >= SETTLE) {
            save(&session::capture(&mut conn)?, path)?;
            changed_at = None;
        }
    }
}

fn changes_session(event: &Event) -> bool {
    match event {
        Event::Window(window) => matches!(
            window.change,
            WindowChange::New
                | WindowChange::Close
                | WindowChange::Move
                | WindowChange::Floating
                | WindowChange::FullscreenMode
                | WindowChange::Focus
        ),
        Event::Workspace(_) => true,
        _ => false,
    }
}

/// Dump the whole session's info
fn dump(session: &Session) -> String {
    let mut out = String::new();
    for Workspace {
        name,
        output,
        layout,
        windows,
    } in &session.workspaces
    {
        out.push_str(&format!(
            "workspace {name} on {output} [{}]\n",
            layout.as_command()
        ));
        for window in windows {
            dump_window(&mut out, window);
        }
    }
    if !session.scratchpad.is_empty() {
        out.push_str("scratchpad\n");
        for window in &session.scratchpad {
            dump_window(&mut out, window);
        }
    }
    out
}

/// Dump a single window's info
fn dump_window(
    out: &mut String,
    Window {
        app_id,
        title,
        argv,
        floating,
        fullscreen,
        focused,
        rect,
    }: &Window,
) {
    let mut flags = Vec::new();
    if *floating {
        flags.push(format!(
            "floating {}x{}+{}+{}",
            rect.width, rect.height, rect.x, rect.y
        ));
    }
    if *fullscreen {
        flags.push("fullscreen".to_string());
    }
    if *focused {
        flags.push("focused".to_string());
    }
    let flags = if flags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", flags.join(", "))
    };
    out.push_str(&format!(
        "  {} {title:?}{flags}\n    {argv:?}\n",
        if app_id.is_empty() { "?" } else { &app_id },
    ));
}
