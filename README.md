# swaymnesia

Saves the layout of a sway session and resurrects it on the next login. The name contradicts the actual funcionality, but i's *cool*.

## Usage

```
usage: swaymnesia <save|restore|watch|dump> [path]

  save     write the current sway session to path
  restore  start the programs recorded in path and lay their windows out
  watch    rewrite path whenever the session changes
  dump     print the session stored in path as text
```

The path defaults to `$XDG_DATA_HOME/swaymnesia/session.swmn`, falling back to
`~/.local/share/swaymnesia/session.swmn`.

To keep a session across logins, add this to the sway config:

```
exec swaymnesia restore && swaymnesia watch
```

## What is stored

### Per workspace
* its name,
* the output it is on,
* its layout

### Per window
* the application id,
* the title,
* the command line the process was started with,
* whether it's floating, fullscreen or focused
* its geometry

A window is only restorable if its command line can be read from `/proc`.
Flatpak apps are recorded as `flatpak run <app-id>`, without their arguments,
since their command line refers to paths inside the sandbox.
Sessions are keyed on the workspace name, so restoring on a different machine
(with different outputs) works.

## Configuration

Rules to determine which windows should be saved to the session are read from
`$XDG_CONFIG_HOME/swaymnesia/config.toml`.

A window is ignored when it matches an `[[ignore]]` rule and no `[[allow]]`
rule. Using `[[allow]]`s can make broad rules have exceptions, like so:

```toml
[[ignore]]
app_id = '^com\.mitchellh\.ghostty$'

[[allow]]
app_id = '^com\.mitchellh\.ghostty$'
title = '^zellij-session-'
```

A rule can have three fields:

* `app_id`: the application id
* `title`: the window title
* `command`: the whole command line

Values are regular expressions.

## Session format

A session is a flat binary file. All integers are little-endian, and a `str` is a
u32 byte length followed by that many bytes of UTF-8.
```
header     magic [4]u8 = "SWMN"
           version u16 = 1
           workspace count u32
workspace  name str
           output str
           layout u8, one of 0 splith, 1 splitv, 2 stacked, 3 tabbed
           window count u32
window     app_id str, empty when the view reports none
           title str
           argv count u32, followed by that many str
           flags u8, bit 0 floating, bit 1 fullscreen, bit 2 focused
           rect i32 x, y, width, height, only meaningful when floating
trailer    scratchpad window count u32
           window, repeated that many times
```

Workspaces and windows appear in the order sway reports them, which is the order
they are recreated in. `swaymnesia dump` prints output of this format.

## Limitations

- Only the workspace layout is kept, not the full split tree, so windows come
  back side by side in their recorded order rather than in nested splits.
- Two windows belonging to one process share a command line, so restoring starts
  that program twice.
- A scratchpad window that was showing at save time is hidden on restore.
