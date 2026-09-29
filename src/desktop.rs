//! The computer-use actions on one Hyprland monitor, in the scaled pixel space its screenshots use.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use serde::Deserialize;

use crate::pointer::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Pointer, Wheel};

/// How long the screen gets to settle before the screenshot that answers an action.
const SETTLE: Duration = Duration::from_millis(400);

#[derive(Deserialize)]
pub struct Input {
    pub action: String,
    pub coordinate: Option<[f64; 2]>,
    pub start_coordinate: Option<[f64; 2]>,
    pub text: Option<String>,
    pub scroll_direction: Option<String>,
    pub scroll_amount: Option<u32>,
    pub duration: Option<f64>,
    pub region: Option<[f64; 4]>,
}

#[derive(Default)]
pub struct Reply {
    pub text: Option<String>,
    pub png: Option<Vec<u8>>,
}

/// One monitor: where it sits in the layout, its logical size, and screenshot pixels per logical pixel.
pub struct Display {
    pub name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    factor: f64,
}

#[derive(Deserialize)]
struct Workspace {
    id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveWorkspace {
    active_workspace: Workspace,
    special_workspace: Workspace,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    address: String,
    mapped: bool,
    hidden: bool,
    floating: bool,
    at: [f64; 2],
    size: [f64; 2],
    workspace: Workspace,
    #[serde(rename = "focusHistoryID")]
    focus_history_id: i64,
}

#[derive(Deserialize)]
struct Monitor {
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    transform: u8,
    focused: bool,
}

fn run(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    match output.status.success() {
        true => Ok(output.stdout),
        false => Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

impl Display {
    /// The monitor named `wanted`, else the focused one; screenshots are at most `max_width` wide.
    pub fn pick(wanted: Option<&str>, max_width: f64) -> Result<Self, String> {
        let monitors: Vec<Monitor> = serde_json::from_slice(&run("hyprctl", &["monitors", "-j"])?)
            .map_err(|e| format!("hyprctl monitors: {e}"))?;
        let monitor = monitors
            .into_iter()
            .find(|m| match wanted {
                Some(name) => m.name == name,
                None => m.focused,
            })
            .ok_or_else(|| format!("no monitor {}", wanted.unwrap_or("is focused")))?;
        let (mut width, mut height) = (
            monitor.width / monitor.scale,
            monitor.height / monitor.scale,
        );
        // A monitor turned a quarter swaps its sides.
        if monitor.transform % 2 == 1 {
            std::mem::swap(&mut width, &mut height);
        }
        Ok(Self {
            name: monitor.name,
            x: monitor.x,
            y: monitor.y,
            factor: (max_width / width).min(1.0),
            width,
            height,
        })
    }

    /// The screenshot size, which is the coordinate space every action speaks.
    pub fn size(&self) -> (u32, u32) {
        (
            (self.width * self.factor).round() as u32,
            (self.height * self.factor).round() as u32,
        )
    }

    fn global(&self, [x, y]: [f64; 2]) -> Result<(i64, i64), String> {
        let (w, h) = self.size();
        if x < 0.0 || y < 0.0 || x > f64::from(w) || y > f64::from(h) {
            return Err(format!("({x}, {y}) is outside the {w}x{h} screen"));
        }
        Ok((
            (self.x + x / self.factor).round() as i64,
            (self.y + y / self.factor).round() as i64,
        ))
    }

    fn local(&self, gx: f64, gy: f64) -> (i64, i64) {
        (
            ((gx - self.x) * self.factor).round() as i64,
            ((gy - self.y) * self.factor).round() as i64,
        )
    }

    /// The whole monitor at screenshot size, cursor included.
    pub fn screenshot(&self) -> Result<Vec<u8>, String> {
        let geometry = format!("{},{} {}x{}", self.x, self.y, self.width, self.height);
        run(
            "grim",
            &["-c", "-g", &geometry, "-s", &self.factor.to_string(), "-"],
        )
    }

    /// One region, `[x1, y1, x2, y2]` in screenshot pixels, at the monitor's full resolution.
    fn zoom(&self, [x1, y1, x2, y2]: [f64; 4]) -> Result<Vec<u8>, String> {
        let (left, top) = self.global([x1.min(x2), y1.min(y2)])?;
        let (right, bottom) = self.global([x1.max(x2), y1.max(y2)])?;
        let geometry = format!(
            "{left},{top} {}x{}",
            (right - left).max(1),
            (bottom - top).max(1)
        );
        run("grim", &["-c", "-g", &geometry, "-"])
    }
}

pub struct Computer {
    display: Display,
    pointer: Option<Pointer>,
}

impl Computer {
    pub fn new(display: Display) -> Self {
        Self {
            display,
            pointer: None,
        }
    }

    pub fn display(&self) -> &Display {
        &self.display
    }

    fn pointer(&mut self) -> Result<&mut Pointer, String> {
        if self.pointer.is_none() {
            self.pointer = Some(Pointer::connect()?);
        }
        Ok(self.pointer.as_mut().expect("just connected"))
    }

    fn move_to(&mut self, at: [f64; 2]) -> Result<(), String> {
        let (x, y) = self.display.global(at)?;
        self.warp(x, y)
    }

    /// Place the cursor at layout coordinates and tell the window beneath it.
    fn warp(&mut self, x: i64, y: i64) -> Result<(), String> {
        // A Lua-configured Hyprland dispatches Lua; an older one still takes the plain dispatcher.
        let lua = format!("hl.dsp.cursor.move({{ x = {x}, y = {y} }})");
        run("hyprctl", &["dispatch", &lua]).or_else(|_| {
            run(
                "hyprctl",
                &["dispatch", "movecursor", &x.to_string(), &y.to_string()],
            )
        })?;
        self.pointer()?.nudge()
    }

    /// Focus the window under the cursor first, since Hyprland spends a click on an unfocused window focusing it.
    fn focus_under_cursor(&mut self) -> Result<(), String> {
        let text = String::from_utf8(run("hyprctl", &["cursorpos"])?).map_err(|e| e.to_string())?;
        let (x, y) = text
            .trim()
            .split_once(',')
            .ok_or_else(|| format!("hyprctl cursorpos said {text}"))?;
        let parse = |v: &str| v.trim().parse::<f64>().map_err(|e| e.to_string());
        let (x, y) = (parse(x)?, parse(y)?);
        let clients: Vec<Client> = serde_json::from_slice(&run("hyprctl", &["clients", "-j"])?)
            .map_err(|e| format!("hyprctl clients: {e}"))?;
        let monitors: Vec<ActiveWorkspace> =
            serde_json::from_slice(&run("hyprctl", &["monitors", "-j"])?)
                .map_err(|e| format!("hyprctl monitors: {e}"))?;
        let shown: Vec<i64> = monitors
            .iter()
            .flat_map(|m| [m.active_workspace.id, m.special_workspace.id])
            .filter(|id| *id != 0)
            .collect();
        let under = clients
            .iter()
            .filter(|c| c.mapped && !c.hidden && shown.contains(&c.workspace.id))
            .filter(|c| {
                x >= c.at[0] && x < c.at[0] + c.size[0] && y >= c.at[1] && y < c.at[1] + c.size[1]
            })
            // Floating windows sit above tiled ones; among equals, the one focused last is on top.
            .min_by_key(|c| (!c.floating, c.focus_history_id));
        let Some(under) = under else {
            return Ok(());
        };
        let active: Option<Client> =
            serde_json::from_slice(&run("hyprctl", &["activewindow", "-j"])?).ok();
        if active.is_some_and(|a| a.address == under.address) {
            return Ok(());
        }
        let window = format!("address:{}", under.address);
        let lua = format!("hl.dsp.focus({{ window = \"{window}\" }})");
        run("hyprctl", &["dispatch", &lua])
            .or_else(|_| run("hyprctl", &["dispatch", "focuswindow", &window]))?;
        sleep(Duration::from_millis(60));
        // Focusing warps the cursor to the window's middle, so it goes back to where the click is meant.
        self.warp(x.round() as i64, y.round() as i64)
    }

    fn cursor(&self) -> Result<(i64, i64), String> {
        let text = String::from_utf8(run("hyprctl", &["cursorpos"])?).map_err(|e| e.to_string())?;
        let (x, y) = text
            .trim()
            .split_once(',')
            .ok_or_else(|| format!("hyprctl cursorpos said {text}"))?;
        let parse = |v: &str| v.trim().parse::<f64>().map_err(|e| e.to_string());
        Ok(self.display.local(parse(x)?, parse(y)?))
    }

    /// Press `button` `clicks` times, with `modifiers` held throughout.
    fn click(&mut self, button: u32, clicks: u32, modifiers: Option<&str>) -> Result<(), String> {
        self.focus_under_cursor()?;
        let held = hold_modifiers(modifiers, 150 + 120 * u64::from(clicks))?;
        for at in 0..clicks {
            if at > 0 {
                sleep(Duration::from_millis(60));
            }
            let pointer = self.pointer()?;
            pointer.button(button, true)?;
            pointer.button(button, false)?;
        }
        release(held)
    }

    pub fn act(&mut self, input: Input) -> Result<Reply, String> {
        let modifiers = input.text.as_deref();
        match input.action.as_str() {
            "screenshot" => return self.shot(None),
            "zoom" => {
                let region = input.region.ok_or("zoom needs a region")?;
                return Ok(Reply {
                    png: Some(self.display.zoom(region)?),
                    ..Default::default()
                });
            }
            "cursor_position" => {
                let (x, y) = self.cursor()?;
                return Ok(Reply {
                    text: Some(format!("X={x},Y={y}")),
                    ..Default::default()
                });
            }
            "mouse_move" => {
                self.move_to(input.coordinate.ok_or("mouse_move needs a coordinate")?)?
            }
            "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
                if let Some(at) = input.coordinate {
                    self.move_to(at)?;
                }
                let (button, clicks) = match input.action.as_str() {
                    "right_click" => (BTN_RIGHT, 1),
                    "middle_click" => (BTN_MIDDLE, 1),
                    "double_click" => (BTN_LEFT, 2),
                    "triple_click" => (BTN_LEFT, 3),
                    _ => (BTN_LEFT, 1),
                };
                self.click(button, clicks, modifiers)?;
            }
            "left_mouse_down" | "left_mouse_up" => {
                if let Some(at) = input.coordinate {
                    self.move_to(at)?;
                }
                let pressed = input.action == "left_mouse_down";
                if pressed {
                    self.focus_under_cursor()?;
                }
                self.pointer()?.button(BTN_LEFT, pressed)?;
            }
            "left_click_drag" => {
                let from = input
                    .start_coordinate
                    .ok_or("left_click_drag needs a start_coordinate")?;
                let to = input
                    .coordinate
                    .ok_or("left_click_drag needs a coordinate")?;
                self.move_to(from)?;
                self.focus_under_cursor()?;
                self.pointer()?.button(BTN_LEFT, true)?;
                // Intermediate moves, so the target sees a drag rather than a jump.
                for step in 1..=8 {
                    let t = f64::from(step) / 8.0;
                    self.move_to([
                        from[0] + (to[0] - from[0]) * t,
                        from[1] + (to[1] - from[1]) * t,
                    ])?;
                    sleep(Duration::from_millis(16));
                }
                self.pointer()?.button(BTN_LEFT, false)?;
            }
            "scroll" => {
                if let Some(at) = input.coordinate {
                    self.move_to(at)?;
                }
                let wheel = match input.scroll_direction.as_deref() {
                    Some("up") => Wheel::Up,
                    Some("down") => Wheel::Down,
                    Some("left") => Wheel::Left,
                    Some("right") => Wheel::Right,
                    other => {
                        return Err(format!(
                            "scroll_direction {other:?} is not up, down, left or right"
                        ));
                    }
                };
                let notches = input.scroll_amount.unwrap_or(3);
                let held = hold_modifiers(modifiers, 150 + 40 * u64::from(notches))?;
                self.pointer()?.scroll(wheel, notches)?;
                release(held)?;
            }
            "key" => {
                let keys = input.text.ok_or("key needs text")?;
                for chord in keys.split_whitespace() {
                    let args = chord_args(chord)?;
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    run("wtype", &args)?;
                }
            }
            "hold_key" => {
                let key = input.text.ok_or("hold_key needs text")?;
                let millis = (input.duration.unwrap_or(1.0) * 1000.0).round() as u64;
                let args = hold_args(&key, millis)?;
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                run("wtype", &args)?;
            }
            "type" => {
                let text = input.text.ok_or("type needs text")?;
                let mut child = Command::new("wtype")
                    .args(["-d", "4", "-"])
                    .stdin(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("wtype: {e}"))?;
                child
                    .stdin
                    .take()
                    .ok_or("wtype took no input")?
                    .write_all(text.as_bytes())
                    .map_err(|e| format!("wtype: {e}"))?;
                let status = child.wait().map_err(|e| format!("wtype: {e}"))?;
                if !status.success() {
                    return Err("wtype failed to type".into());
                }
            }
            "wait" => sleep(Duration::from_secs_f64(
                input.duration.unwrap_or(1.0).clamp(0.0, 100.0),
            )),
            other => return Err(format!("unknown action {other}")),
        }
        self.shot(Some(SETTLE))
    }

    fn shot(&self, settle: Option<Duration>) -> Result<Reply, String> {
        if let Some(settle) = settle {
            sleep(settle);
        }
        Ok(Reply {
            png: Some(self.display.screenshot()?),
            ..Default::default()
        })
    }
}

/// wtype's name for an xdotool modifier, or `None` when `name` is an ordinary key.
fn modifier(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "control_l" | "control_r" => "ctrl",
        "shift" | "shift_l" | "shift_r" => "shift",
        "alt" | "alt_l" | "alt_r" | "option" => "alt",
        "super" | "super_l" | "super_r" | "cmd" | "command" | "meta" | "win" | "logo" => "logo",
        "altgr" => "altgr",
        _ => return None,
    })
}

/// The xkb keysym an xdotool key name means.
fn keysym(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Return".into(),
        "esc" | "escape" => "Escape".into(),
        "backspace" => "BackSpace".into(),
        "tab" => "Tab".into(),
        "space" => "space".into(),
        "del" | "delete" => "Delete".into(),
        "ins" | "insert" => "Insert".into(),
        "pageup" | "page_up" | "prior" => "Page_Up".into(),
        "pagedown" | "page_down" | "next" => "Page_Down".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "ctrl" | "control" => "Control_L".into(),
        "shift" => "Shift_L".into(),
        "alt" => "Alt_L".into(),
        "super" | "cmd" | "meta" | "win" | "logo" => "Super_L".into(),
        lower if lower.len() > 1 && lower.starts_with('f') && lower[1..].parse::<u8>().is_ok() => {
            lower.to_uppercase()
        }
        _ => name.into(),
    }
}

/// `ctrl+shift+t` as wtype arguments: the modifiers pressed around one typed key.
fn chord_args(chord: &str) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = chord.split('+').filter(|p| !p.is_empty()).collect();
    let (key, mods) = parts
        .split_last()
        .ok_or_else(|| format!("{chord:?} names no key"))?;
    let mods = mods
        .iter()
        .map(|m| modifier(m).ok_or_else(|| format!("{m} is not a modifier")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut args = Vec::new();
    for m in &mods {
        args.extend(["-M".to_string(), m.to_string()]);
    }
    args.extend(["-k".to_string(), keysym(key)]);
    for m in mods.iter().rev() {
        args.extend(["-m".to_string(), m.to_string()]);
    }
    Ok(args)
}

/// Hold one key or chord for `millis`.
fn hold_args(chord: &str, millis: u64) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = chord.split('+').filter(|p| !p.is_empty()).collect();
    let (key, mods) = parts
        .split_last()
        .ok_or_else(|| format!("{chord:?} names no key"))?;
    let mut args = Vec::new();
    for m in mods {
        let m = modifier(m).ok_or_else(|| format!("{m} is not a modifier"))?;
        args.extend(["-M".to_string(), m.to_string()]);
    }
    match modifier(key) {
        Some(m) => args.extend([
            "-M".to_string(),
            m.to_string(),
            "-s".to_string(),
            millis.to_string(),
            "-m".to_string(),
            m.to_string(),
        ]),
        None => args.extend([
            "-P".to_string(),
            keysym(key),
            "-s".to_string(),
            millis.to_string(),
            "-p".to_string(),
            keysym(key),
        ]),
    }
    Ok(args)
}

/// Modifiers held by a background wtype for `millis`, since they release when it exits.
fn hold_modifiers(
    modifiers: Option<&str>,
    millis: u64,
) -> Result<Option<std::process::Child>, String> {
    let Some(modifiers) = modifiers.filter(|m| !m.trim().is_empty()) else {
        return Ok(None);
    };
    let mods = modifiers
        .split('+')
        .map(|m| modifier(m.trim()).ok_or_else(|| format!("{m} is not a modifier")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut args = Vec::new();
    for m in &mods {
        args.extend(["-M".to_string(), m.to_string()]);
    }
    args.extend(["-s".to_string(), millis.to_string()]);
    for m in mods.iter().rev() {
        args.extend(["-m".to_string(), m.to_string()]);
    }
    let child = Command::new("wtype")
        .args(&args)
        .spawn()
        .map_err(|e| format!("wtype: {e}"))?;
    // Give the compositor the press before the click that depends on it.
    sleep(Duration::from_millis(80));
    Ok(Some(child))
}

fn release(held: Option<std::process::Child>) -> Result<(), String> {
    if let Some(mut child) = held {
        child.wait().map_err(|e| format!("wtype: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_presses_its_modifiers_around_the_key() {
        assert_eq!(
            chord_args("ctrl+shift+t").unwrap(),
            [
                "-M", "ctrl", "-M", "shift", "-k", "t", "-m", "shift", "-m", "ctrl"
            ]
        );
        assert_eq!(chord_args("Return").unwrap(), ["-k", "Return"]);
        assert_eq!(chord_args("super").unwrap(), ["-k", "Super_L"]);
        assert_eq!(chord_args("f5").unwrap(), ["-k", "F5"]);
        assert!(chord_args("tab+x").is_err());
    }

    #[test]
    fn a_held_key_sleeps_between_press_and_release() {
        assert_eq!(
            hold_args("shift", 500).unwrap(),
            ["-M", "shift", "-s", "500", "-m", "shift"]
        );
        assert_eq!(
            hold_args("ctrl+a", 200).unwrap(),
            ["-M", "ctrl", "-P", "a", "-s", "200", "-p", "a"]
        );
    }

    #[test]
    fn coordinates_scale_back_to_the_layout() {
        let display = Display {
            name: "DP-4".into(),
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            factor: 1280.0 / 1920.0,
        };
        assert_eq!(display.size(), (1280, 720));
        assert_eq!(display.global([640.0, 360.0]).unwrap(), (960, 540));
        assert_eq!(display.local(960.0, 540.0), (640, 360));
        assert!(display.global([1300.0, 10.0]).is_err());
    }
}
