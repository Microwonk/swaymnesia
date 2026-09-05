use core::fmt;
use std::{
    error::Error,
    fs,
    path::Path,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use swayipc::{Connection as SwayConnection, Event, EventType, WindowChange};

use crate::{config::Config, session::Session};

pub mod config;
pub mod format;
pub mod restore;
pub mod session;

/// How long the session has to stay unchanged before `watch` writes it to the session file
const SETTLE: Duration = Duration::from_secs(1);

/// Writes through a temporary file for an atomic write operation
pub fn save_session(session: &Session, path: &Path) -> Result<(), Box<dyn Error>> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, session.encode())?;
    fs::rename(&temporary, path)?;
    Ok(())
}

pub fn load_session(path: &Path) -> Result<Session, Box<dyn Error>> {
    let context = |e: &dyn fmt::Display| format!("{}: {e}", path.display());
    let bytes = fs::read(path).map_err(|e| context(&e))?;
    Session::decode(&bytes).map_err(|e| context(&e).into())
}

/// Watches the session for as long as sway runs, saving when settled
pub fn watch(path: &Path) -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
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
            save_session(&session::capture(&mut conn, &config)?, path)?;
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
