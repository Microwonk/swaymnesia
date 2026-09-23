use crate::session::{Layout, Session, Tile, Window};
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

/// Marks the window that the next one is moved behind while the tiling tree is rebuilt
const MARK: &str = "_swaymnesia";

/// A restored tile whose windows all appeared, identified by their sway container ids
enum Placed {
    Window(i64),
    Split(Layout, Vec<Placed>),
}

impl Placed {
    fn first_window(&self) -> i64 {
        match self {
            Placed::Window(id) => *id,
            Placed::Split(_, children) => children[0].first_window(),
        }
    }
}

/// Windows whose state is applied once every window has been started
#[derive(Default)]
struct Pending<'a> {
    floating: Vec<(i64, &'a Window)>,
    scratchpad: Vec<i64>,
    fullscreen: Vec<i64>,
    focused: Option<i64>,
}

pub fn restore(session: &Session) -> Result<(), Box<dyn Error>> {
    let new_windows = watch_new_windows()?;
    let mut conn = SwayConnection::new()?;
    let mut pending = Pending::default();

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

        let mut tiles = Vec::new();
        for tile in &workspace.tiles {
            if let Some(placed) =
                spawn_tile(&mut conn, &new_windows, tile, &workspace.name, &mut pending)?
            {
                tiles.push(placed);
            }
        }
        // The windows were opened side by side on the workspace, nest them into their splits
        line_up(&mut conn, &tiles);
        for tile in &tiles {
            arrange(&mut conn, tile);
        }

        for window in &workspace.floating {
            if let Some(id) = spawn_window(
                &mut conn,
                &new_windows,
                window,
                &workspace.name,
                &mut pending,
            )? {
                run(&mut conn, &format!("[con_id={id}] floating enable"));
                pending.floating.push((id, window));
            }
        }
    }
    run(&mut conn, &format!("unmark {MARK}"));

    for window in &session.scratchpad {
        let Some(id) = spawn_and_wait(&new_windows, window)? else {
            continue;
        };
        run(&mut conn, &format!("[con_id={id}] floating enable"));
        pending.floating.push((id, window));
        pending.scratchpad.push(id);
    }

    thread::sleep(SETTLE);
    for (id, window) in pending.floating {
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

    for id in pending.scratchpad {
        run(&mut conn, &format!("[con_id={id}] move scratchpad"));
    }

    for id in pending.fullscreen {
        run(&mut conn, &format!("[con_id={id}] fullscreen enable"));
    }

    if let Some(id) = pending.focused {
        run(&mut conn, &format!("[con_id={id}] focus"));
    }

    Ok(())
}

/// Starts every window of `tile`, leaving out the windows that did not appear and the splits
/// that became empty
fn spawn_tile<'a>(
    conn: &mut SwayConnection,
    new_windows: &Receiver<i64>,
    tile: &'a Tile,
    workspace: &str,
    pending: &mut Pending<'a>,
) -> Result<Option<Placed>, Box<dyn Error>> {
    Ok(match tile {
        Tile::Window(window) => {
            spawn_window(conn, new_windows, window, workspace, pending)?.map(Placed::Window)
        }
        Tile::Split { layout, children } => {
            let mut placed = Vec::new();
            for child in children {
                if let Some(child) = spawn_tile(conn, new_windows, child, workspace, pending)? {
                    placed.push(child);
                }
            }
            match placed.len() {
                0 | 1 => placed.pop(),
                _ => Some(Placed::Split(*layout, placed)),
            }
        }
    })
}

/// Starts `window` on `workspace` and returns its container id
fn spawn_window<'a>(
    conn: &mut SwayConnection,
    new_windows: &Receiver<i64>,
    window: &'a Window,
    workspace: &str,
    pending: &mut Pending<'a>,
) -> Result<Option<i64>, Box<dyn Error>> {
    let Some(id) = spawn_and_wait(new_windows, window)? else {
        return Ok(None);
    };
    run(
        conn,
        &format!(
            "[con_id={id}] move container to workspace {}",
            quote(workspace)
        ),
    );
    if window.fullscreen {
        pending.fullscreen.push(id);
    }
    if window.focused {
        pending.focused = Some(id);
    }
    Ok(Some(id))
}

/// Turns the first window of `tile` into the split it stands for, then does the same for each
/// child of the split.
///
/// Every window of `tile` has to be a sibling of its first window, in the order of the tree.
fn arrange(conn: &mut SwayConnection, tile: &Placed) {
    let Placed::Split(layout, children) = tile else {
        return;
    };
    let first = tile.first_window();
    run(
        conn,
        &format!("[con_id={first}] {}", layout.split_command()),
    );
    // Applied to a window, `layout` changes the container the window is in
    run(
        conn,
        &format!("[con_id={first}] layout {}", layout.as_command()),
    );
    line_up(conn, children);
    for child in children {
        arrange(conn, child);
    }
}

/// Moves the first window of each tile right behind the first window of the tile before it.
///
/// Moving a container to a marked window inserts it next to that window, so this gathers the
/// tiles in the container of the first one, in their order.
fn line_up(conn: &mut SwayConnection, tiles: &[Placed]) {
    for pair in tiles.windows(2) {
        let (behind, window) = (pair[0].first_window(), pair[1].first_window());
        run(conn, &format!("[con_id={behind}] mark {MARK}"));
        run(
            conn,
            &format!("[con_id={window}] move container to mark {MARK}"),
        );
    }
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
