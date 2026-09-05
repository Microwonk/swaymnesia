use crate::config::Config;
use crate::format;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use swayipc::{Connection, Node, NodeLayout, NodeType, ScratchpadState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub workspaces: Vec<Workspace>,
    pub scratchpad: Vec<Window>,
}

impl Session {
    pub fn default_path() -> PathBuf {
        let data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share")
            });
        data_home.join("swaymnesia/session.swmn")
    }

    pub fn encode(&self) -> Vec<u8> {
        format::encode(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Session, crate::format::Error> {
        format::decode(bytes)
    }

    /// Dump the whole session's info
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for Workspace {
            name,
            output,
            layout,
            windows,
        } in &self.workspaces
        {
            out.push_str(&format!(
                "workspace {name} on {output} [{}]\n",
                layout.as_command()
            ));
            for window in windows {
                dump_window(&mut out, window);
            }
        }
        if !self.scratchpad.is_empty() {
            out.push_str("scratchpad\n");
            for window in &self.scratchpad {
                dump_window(&mut out, window);
            }
        }
        out
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub output: String,
    pub layout: Layout,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub app_id: String,
    pub title: String,
    /// The command line the process was started with, read from /proc
    pub argv: Vec<String>,
    pub floating: bool,
    pub fullscreen: bool,
    pub focused: bool,
    /// Position and size of floating window
    pub rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Layout {
    SplitH = 0,
    SplitV = 1,
    Stacked = 2,
    Tabbed = 3,
}

impl Layout {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Layout::SplitH),
            1 => Some(Layout::SplitV),
            2 => Some(Layout::Stacked),
            3 => Some(Layout::Tabbed),
            _ => None,
        }
    }

    /// The name sway's `layout` command uses
    pub fn as_command(self) -> &'static str {
        match self {
            Layout::SplitH => "splith",
            Layout::SplitV => "splitv",
            Layout::Stacked => "stacking",
            Layout::Tabbed => "tabbed",
        }
    }

    fn from_node(layout: NodeLayout) -> Self {
        match layout {
            NodeLayout::SplitV => Layout::SplitV,
            NodeLayout::Stacked => Layout::Stacked,
            NodeLayout::Tabbed => Layout::Tabbed,
            _ => Layout::SplitH,
        }
    }
}

/// Captures every workspace with at least one window, as well as any scratchpads
pub fn capture(conn: &mut Connection, config: &Config) -> Result<Session, swayipc::Error> {
    let tree = conn.get_tree()?;
    let mut workspaces = Vec::new();
    let mut scratchpad = Vec::new();

    for output in &tree.nodes {
        for workspace in &output.nodes {
            let mut windows = Vec::new();
            collect(workspace, false, &mut windows, &mut scratchpad, config);

            let name = workspace.name.clone().unwrap_or_default();
            if name.starts_with("__i3") || windows.is_empty() {
                continue;
            }

            workspaces.push(Workspace {
                name,
                output: output.name.clone().unwrap_or_default(),
                layout: Layout::from_node(workspace.layout),
                windows,
            });
        }
    }
    Ok(Session {
        workspaces,
        scratchpad,
    })
}

/// Recurses through [node], appending every leaf node into [`windows`]/[`scratchpad`]
fn collect(
    node: &Node,
    floating: bool,
    windows: &mut Vec<Window>,
    scratchpad: &mut Vec<Window>,
    config: &Config,
) {
    let floating = floating || matches!(node.node_type, NodeType::FloatingCon);
    if let Some(window) = view(node, floating, config) {
        if is_scratchpad(node) {
            scratchpad
        } else {
            windows
        }
        .push(window);
        return;
    }
    for child in &node.nodes {
        collect(child, floating, windows, scratchpad, config);
    }
    for child in &node.floating_nodes {
        collect(child, true, windows, scratchpad, config);
    }
}

fn is_scratchpad(node: &Node) -> bool {
    !matches!(node.scratchpad_state, None | Some(ScratchpadState::None))
}

fn view(node: &Node, floating: bool, config: &Config) -> Option<Window> {
    if !node.nodes.is_empty() || !node.floating_nodes.is_empty() {
        return None;
    }
    let argv = node.pid.and_then(cmdline)?;
    let app_id = node
        .app_id
        .clone()
        .or_else(|| node.window_properties.as_ref()?.class.clone())
        .unwrap_or_default();

    let title = node.name.clone().unwrap_or_default();
    if config.skips(&app_id, &title, &argv.join(" ")) {
        return None;
    }

    Some(Window {
        app_id,
        title,
        argv,
        floating,
        fullscreen: matches!(node.fullscreen_mode, Some(1 | 2)),
        focused: node.focused,
        rect: Rect {
            x: node.rect.x,
            y: node.rect.y,
            width: node.rect.width,
            height: node.rect.height,
        },
    })
}

/// Read argv from `/proc` via [pid]
fn cmdline(pid: i32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    (!argv.is_empty()).then(|| repair_argv(argv))
}

/// Recovers an argument vector from a process that overwrote its own argv area.
///
/// For example: "/usr/lib/signal-desktop/signal-desktop --" with the separating NUL gone
fn repair_argv(argv: Vec<String>) -> Vec<String> {
    let [only] = argv.as_slice() else {
        return argv;
    };
    if is_program(only) {
        return argv;
    }
    let split: Vec<String> = only.split_whitespace().map(str::to_string).collect();
    match split.first() {
        Some(program) if is_program(program) => split,
        _ => argv,
    }
}

/// Whether a name can be started, either as a path or through a lookup in $PATH.
fn is_program(name: &str) -> bool {
    if name.contains('/') {
        return is_executable(Path::new(name));
    }
    env::var_os("PATH")
        .is_some_and(|path| env::split_paths(&path).any(|dir| is_executable(&dir.join(name))))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}
