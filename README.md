# hypr-computer-mcp

Anthropic's computer-use tool as a stdio MCP server that drives a Hyprland
desktop. It exposes one tool, `computer`, with the same actions and parameters
as the API's built-in one: `screenshot`, `zoom`, `left_click`, `double_click`,
`triple_click`, `right_click`, `middle_click`, `left_click_drag`,
`left_mouse_down`/`up`, `mouse_move`, `scroll`, `key`, `hold_key`, `type`,
`cursor_position`, `wait`.

Claude wrote this code. It also created this repository and pushed it on its
own through this server: it filled GitHub's new-repository form in the browser
and typed the git commands into a terminal, working from screenshots.

## How it works

- Screenshots come from `grim`, scaled so the monitor is at most 1280 pixels
  wide. Every coordinate is in that screenshot's pixels.
- The cursor is placed with `hyprctl dispatch`. It tries the Lua form
  (`hl.dsp.cursor.move`) first and falls back to `movecursor`.
- Buttons and the wheel go through a `zwlr_virtual_pointer_v1`. Since a button
  carries no position, every warp is followed by a net-zero 1px wiggle so the
  client sees where the pointer is.
- A click focuses the window under the cursor first; otherwise Hyprland spends
  the click on focusing it.
- Keys and text go through a `zwp_virtual_keyboard_v1` laid out like a US
  keyboard, so each character arrives on the key it has there. wtype numbers
  keys in the order characters first appear, and Chromium reads punctuation by
  key position, so a `-` that happened to land on Backspace's code erased the
  character before it. Key names use xdotool syntax (`Return`,
  `ctrl+shift+t`).
- While it's in use, it holds an `org.freedesktop.ScreenSaver` inhibit and a
  logind `idle:sleep` block, so the screen doesn't lock or blank and the
  machine doesn't suspend under it. Both are released after a quiet spell. If
  the monitor is off when an action comes in, it nudges the pointer the way a
  person would and waits for the monitor to come back.

## Setup

Needs Hyprland, and `grim` on `PATH`.

```sh
cargo build --release
claude mcp add --scope user hypr-computer -- "$PWD/target/release/hypr-computer-mcp"
```

Claude Code reserves the server name `computer-use`, hence `hypr-computer`.

| Variable | Default | |
| --- | --- | --- |
| `HYPR_COMPUTER_MONITOR` | the focused one | Monitor to drive, e.g. `DP-3` |
| `HYPR_COMPUTER_MAX_WIDTH` | `1280` | Screenshot width cap |
| `HYPR_COMPUTER_AWAKE_MINUTES` | `15` | How long to hold the session awake after the last action; `0` turns it off |

One action can also be run from a shell, which writes the screenshot to a file:

```sh
hypr-computer-mcp call '{"action":"left_click","coordinate":[640,360]}' shot.png
```

## Limits

- One monitor per server.
- It can't unlock a locked session.
