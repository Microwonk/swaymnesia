use crate::session::{Session, Window};
use std::error::Error;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;
use swayipc::{Connection as SwayConnection, Event, EventType, WindowChange};

/// How long a respawned program gets to map its first window before it is given up on
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to let the last windows settle before giving them their final geometry
const SETTLE: Duration = Duration::from_millis(500);

pub fn restore(session: &Session) -> Result<(), Box<dyn Error>> {
    let new_windows = watch_new_windows()?;
    let mut conn = SwayConnection::new()?;

    let mut floating = Vec::new();
    let mut scratchpad = Vec::new();
    let mut fullscreen = Vec::new();
    let mut focused = None;

    for workspace in &session.workspaces {
        run(
            &mut conn,
            &format!(
                "workspace --no-auto-back-and-forth {}",
                quote(&workspace.name)
            ),
        );
        if !workspace.output.is_empty() {
            run(
                &mut conn,
                &format!("move workspace to output {}", quote(&workspace.output)),
            );
        }
        run(
            &mut conn,
            &format!("layout {}", workspace.layout.as_command()),
        );

        for window in &workspace.windows {
            let Some(id) = spawn_and_wait(&new_windows, window)? else {
                continue;
            };

            run(
                &mut conn,
                &format!(
                    "[con_id={id}] move container to workspace {}",
                    quote(&workspace.name)
                ),
            );
            if window.floating {
                run(&mut conn, &format!("[con_id={id}] floating enable"));
                floating.push((id, window));
            }
            if window.fullscreen {
                fullscreen.push(id);
            }
            if window.focused {
                focused = Some(id);
            }
        }
    }

    for window in &session.scratchpad {
        let Some(id) = spawn_and_wait(&new_windows, window)? else {
            continue;
        };
        run(&mut conn, &format!("[con_id={id}] floating enable"));
        floating.push((id, window));
        scratchpad.push(id);
    }

    thread::sleep(SETTLE);
    for (id, window) in floating {
        run(
            &mut conn,
            &format!(
                "[con_id={id}] resize set width {} px height {} px",
                window.rect.width, window.rect.height
            ),
        );
        run(
            &mut conn,
            &format!(
                "[con_id={id}] move absolute position {} {}",
                window.rect.x, window.rect.y
            ),
        );
    }

    for id in scratchpad {
        run(&mut conn, &format!("[con_id={id}] move scratchpad"));
    }

    for id in fullscreen {
        run(&mut conn, &format!("[con_id={id}] fullscreen enable"));
    }

    if let Some(id) = focused {
        run(&mut conn, &format!("[con_id={id}] focus"));
    }

    Ok(())
}

/// Starts the program which was recorded for `window` and returns the id of the window from sway.
/// A program that cannot be started or never maps to a window is reported and skipped.
fn spawn_and_wait(
    new_windows: &Receiver<i64>,
    window: &Window,
) -> Result<Option<i64>, Box<dyn Error>> {
    // Windows that appeared for an earlier program, or for something the user started, must not
    // be mistaken for this one.
    while new_windows.try_recv().is_ok() {}

    if let Err(e) = spawn(&window.argv) {
        eprintln!("swaymnesia: cannot start {:?}: {e}", window.argv);
        return Ok(None);
    }
    match new_windows.recv_timeout(SPAWN_TIMEOUT) {
        Ok(id) => Ok(Some(id)),
        Err(RecvTimeoutError::Timeout) => {
            eprintln!("swaymnesia: no window appeared for {:?}", window.argv);
            Ok(None)
        }
        Err(RecvTimeoutError::Disconnected) => Err("sway closed the event stream".into()),
    }
}

/// Subscribes to window events on their own connection and reports the id of each new window
fn watch_new_windows() -> Result<Receiver<i64>, Box<dyn Error>> {
    let events = SwayConnection::new()?.subscribe([EventType::Window])?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for event in events.flatten() {
            if let Event::Window(window) = event
                && matches!(window.change, WindowChange::New)
                && tx.send(window.container.id).is_err()
            {
                return;
            }
        }
    });
    Ok(rx)
}

/// Spawn program without using sway's `exec`
fn spawn(argv: &[String]) -> std::io::Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Ok(());
    };
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// Best-effort restore, i.e. not err'ing if it fails here
fn run(conn: &mut SwayConnection, command: &str) {
    match conn.run_command(command) {
        Ok(outcomes) => {
            for outcome in outcomes {
                if let Err(e) = outcome {
                    eprintln!("swaymnesia: {command}: {e}");
                }
            }
        }
        Err(e) => eprintln!("swaymnesia: {command}: {e}"),
    }
}

fn quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
