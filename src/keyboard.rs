//! Keys and text through a virtual keyboard laid out like a US one, so every client reads a key the way it would read a person's.

use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::thread::sleep;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

/// Pause between typed characters, which some clients need to keep up.
const KEY_GAP: Duration = Duration::from_millis(4);

/// The printable keys: evdev code, then the character and keysym name of the plain and the shifted level.
const PRINTABLE: &[(u32, char, &str, char, &str)] = &[
    (2, '1', "1", '!', "exclam"),
    (3, '2', "2", '@', "at"),
    (4, '3', "3", '#', "numbersign"),
    (5, '4', "4", '$', "dollar"),
    (6, '5', "5", '%', "percent"),
    (7, '6', "6", '^', "asciicircum"),
    (8, '7', "7", '&', "ampersand"),
    (9, '8', "8", '*', "asterisk"),
    (10, '9', "9", '(', "parenleft"),
    (11, '0', "0", ')', "parenright"),
    (12, '-', "minus", '_', "underscore"),
    (13, '=', "equal", '+', "plus"),
    (16, 'q', "q", 'Q', "Q"),
    (17, 'w', "w", 'W', "W"),
    (18, 'e', "e", 'E', "E"),
    (19, 'r', "r", 'R', "R"),
    (20, 't', "t", 'T', "T"),
    (21, 'y', "y", 'Y', "Y"),
    (22, 'u', "u", 'U', "U"),
    (23, 'i', "i", 'I', "I"),
    (24, 'o', "o", 'O', "O"),
    (25, 'p', "p", 'P', "P"),
    (26, '[', "bracketleft", '{', "braceleft"),
    (27, ']', "bracketright", '}', "braceright"),
    (30, 'a', "a", 'A', "A"),
    (31, 's', "s", 'S', "S"),
    (32, 'd', "d", 'D', "D"),
    (33, 'f', "f", 'F', "F"),
    (34, 'g', "g", 'G', "G"),
    (35, 'h', "h", 'H', "H"),
    (36, 'j', "j", 'J', "J"),
    (37, 'k', "k", 'K', "K"),
    (38, 'l', "l", 'L', "L"),
    (39, ';', "semicolon", ':', "colon"),
    (40, '\'', "apostrophe", '"', "quotedbl"),
    (41, '`', "grave", '~', "asciitilde"),
    (43, '\\', "backslash", '|', "bar"),
    (44, 'z', "z", 'Z', "Z"),
    (45, 'x', "x", 'X', "X"),
    (46, 'c', "c", 'C', "C"),
    (47, 'v', "v", 'V', "V"),
    (48, 'b', "b", 'B', "B"),
    (49, 'n', "n", 'N', "N"),
    (50, 'm', "m", 'M', "M"),
    (51, ',', "comma", '<', "less"),
    (52, '.', "period", '>', "greater"),
    (53, '/', "slash", '?', "question"),
];

/// Keys that type no character, by evdev code and keysym name.
const NAMED: &[(u32, &str)] = &[
    (1, "Escape"),
    (14, "BackSpace"),
    (15, "Tab"),
    (28, "Return"),
    (57, "space"),
    (29, "Control_L"),
    (42, "Shift_L"),
    (56, "Alt_L"),
    (125, "Super_L"),
    (100, "ISO_Level3_Shift"),
    (59, "F1"),
    (60, "F2"),
    (61, "F3"),
    (62, "F4"),
    (63, "F5"),
    (64, "F6"),
    (65, "F7"),
    (66, "F8"),
    (67, "F9"),
    (68, "F10"),
    (87, "F11"),
    (88, "F12"),
    (99, "Print"),
    (102, "Home"),
    (103, "Up"),
    (104, "Page_Up"),
    (105, "Left"),
    (106, "Right"),
    (107, "End"),
    (108, "Down"),
    (109, "Page_Down"),
    (110, "Insert"),
    (111, "Delete"),
    (119, "Pause"),
    (127, "Menu"),
];

/// Codes no US key uses, for keysyms beyond it; unassigned ones first, as clients read the least into them.
const SPARE: &[u32] = &[
    84, 195, 196, 197, 198, 199, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Super,
    AltGr,
}

impl Modifier {
    /// The xdotool or wtype name of a modifier.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "shift" | "shift_l" | "shift_r" => Self::Shift,
            "ctrl" | "control" | "control_l" | "control_r" => Self::Ctrl,
            "alt" | "alt_l" | "alt_r" | "option" => Self::Alt,
            "super" | "super_l" | "super_r" | "cmd" | "command" | "meta" | "win" | "logo" => {
                Self::Super
            }
            "altgr" | "iso_level3_shift" => Self::AltGr,
            _ => return None,
        })
    }

    fn code(self) -> u32 {
        match self {
            Self::Shift => 42,
            Self::Ctrl => 29,
            Self::Alt => 56,
            Self::Super => 125,
            Self::AltGr => 100,
        }
    }

    /// Its bit among the real modifiers, which `include "complete"` orders Shift, Lock, Control, Mod1…Mod5.
    fn mask(self) -> u32 {
        match self {
            Self::Shift => 1,
            Self::Ctrl => 1 << 2,
            Self::Alt => 1 << 3,
            Self::Super => 1 << 6,
            Self::AltGr => 1 << 7,
        }
    }
}

/// One key to press: its evdev code and whether it takes Shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stroke {
    code: u32,
    shift: bool,
}

/// What a keysym beyond the US layout is called in a keymap.
fn unicode_name(c: char) -> String {
    format!("U{:04X}", u32::from(c))
}

fn printable(c: char) -> Option<Stroke> {
    PRINTABLE.iter().find_map(|&(code, plain, _, shifted, _)| {
        (c == plain || c == shifted).then_some(Stroke {
            code,
            shift: c == shifted,
        })
    })
}

/// A key by keysym name on the US part of the keymap, if it's there.
fn us_key(name: &str) -> Option<Stroke> {
    if let Some(&(code, _)) = NAMED.iter().find(|(_, n)| *n == name) {
        return Some(Stroke { code, shift: false });
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return printable(c);
    }
    PRINTABLE.iter().find_map(|&(code, _, plain, _, shifted)| {
        (name == plain || name == shifted).then_some(Stroke {
            code,
            shift: name == shifted,
        })
    })
}

/// The xkb keymap: the US keys, plus `extra` keysyms on spare codes.
fn keymap(extra: &[String]) -> String {
    let mut codes = String::new();
    let mut symbols = String::new();
    let mut key = |code: u32, syms: &str| {
        let xkb = code + 8;
        codes.push_str(&format!("<I{xkb}> = {xkb};\n"));
        symbols.push_str(&format!("key <I{xkb}> {{ [ {syms} ] }};\n"));
    };
    for &(code, plain, _, shifted, _) in PRINTABLE {
        key(
            code,
            &format!("{}, {}", unicode_name(plain), unicode_name(shifted)),
        );
    }
    for &(code, name) in NAMED {
        key(code, name);
    }
    for (code, name) in SPARE.iter().zip(extra) {
        key(*code, name);
    }
    format!(
        "xkb_keymap {{\n\
         xkb_keycodes \"hypr-computer\" {{\nminimum = 8;\nmaximum = 255;\n{codes}}};\n\
         xkb_types \"hypr-computer\" {{ include \"complete\" }};\n\
         xkb_compatibility \"hypr-computer\" {{ include \"complete\" }};\n\
         xkb_symbols \"hypr-computer\" {{\n{symbols}\
         modifier_map Shift {{ <I50> }};\n\
         modifier_map Control {{ <I37> }};\n\
         modifier_map Mod1 {{ <I64> }};\n\
         modifier_map Mod4 {{ <I133> }};\n\
         modifier_map Mod5 {{ <I108> }};\n\
         }};\n\
         }};\n"
    )
}

/// A keysym name xkb would take, so a stray one can't spoil the keymap.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

pub struct Keyboard {
    queue: EventQueue<State>,
    keyboard: ZwpVirtualKeyboardV1,
    /// The keysyms on spare codes in the keymap the compositor has now.
    extra: Vec<String>,
    held: u32,
    epoch: Instant,
}

impl Keyboard {
    pub fn connect() -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
        let (globals, queue) =
            registry_queue_init::<State>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
        let handle = queue.handle();
        let seat: WlSeat = globals
            .bind(&handle, 1..=1, ())
            .map_err(|e| format!("the compositor offers no seat: {e}"))?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&handle, 1..=1, ())
            .map_err(|e| format!("the compositor offers no virtual keyboard: {e}"))?;
        let keyboard = manager.create_virtual_keyboard(&seat, &handle, ());
        let mut this = Self {
            queue,
            keyboard,
            extra: Vec::new(),
            held: 0,
            epoch: Instant::now(),
        };
        this.upload()?;
        Ok(this)
    }

    fn now(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }

    fn flush(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut State)
            .map(|_| ())
            .map_err(|e| format!("wayland: {e}"))
    }

    fn upload(&mut self) -> Result<(), String> {
        let mut text = keymap(&self.extra).into_bytes();
        text.push(0);
        let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
        let path = std::path::Path::new(&dir).join(format!(
            "hypr-computer-keymap-{}-{}",
            std::process::id(),
            self.now()
        ));
        let mut file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("keymap {}: {e}", path.display()))?;
        let _ = std::fs::remove_file(&path);
        file.write_all(&text).map_err(|e| format!("keymap: {e}"))?;
        self.keyboard.keymap(
            wl_keyboard::KeymapFormat::XkbV1.into(),
            file.as_fd(),
            text.len() as u32,
        );
        self.flush()
    }

    /// Where a keysym sits, putting it on a spare code (and sending a new keymap) when the US keys lack it.
    fn place(&mut self, name: &str) -> Result<Stroke, String> {
        if let Some(stroke) = us_key(name) {
            return Ok(stroke);
        }
        if !plain_name(name) {
            return Err(format!("{name:?} is not a key"));
        }
        let at = match self.extra.iter().position(|n| n == name) {
            Some(at) => at,
            None => {
                if self.extra.len() == SPARE.len() {
                    self.extra.clear();
                }
                self.extra.push(name.to_string());
                self.upload()?;
                self.extra.len() - 1
            }
        };
        Ok(Stroke {
            code: SPARE[at],
            shift: false,
        })
    }

    fn key(&mut self, code: u32, pressed: bool) {
        let state = match pressed {
            true => wl_keyboard::KeyState::Pressed,
            false => wl_keyboard::KeyState::Released,
        };
        self.keyboard.key(self.now(), code, state.into());
    }

    fn set_mods(&mut self, mask: u32) {
        self.held = mask;
        self.keyboard.modifiers(mask, 0, 0, 0);
    }

    pub fn press_mods(&mut self, mods: &[Modifier]) -> Result<(), String> {
        for m in mods {
            self.key(m.code(), true);
            self.set_mods(self.held | m.mask());
        }
        self.flush()
    }

    pub fn release_mods(&mut self, mods: &[Modifier]) -> Result<(), String> {
        for m in mods.iter().rev() {
            self.key(m.code(), false);
            self.set_mods(self.held & !m.mask());
        }
        self.flush()
    }

    fn stroke(&mut self, stroke: Stroke, down: bool, up: bool) -> Result<(), String> {
        let shift = stroke.shift && self.held & Modifier::Shift.mask() == 0;
        if shift && down {
            self.press_mods(&[Modifier::Shift])?;
        }
        if down {
            self.key(stroke.code, true);
        }
        if up {
            self.key(stroke.code, false);
        }
        if shift && up {
            self.release_mods(&[Modifier::Shift])?;
        }
        self.flush()
    }

    /// Tap the key a keysym name (`Return`, `a`, `minus`, `XF86AudioPlay`) means, with `mods` held around it.
    pub fn tap(&mut self, name: &str, mods: &[Modifier]) -> Result<(), String> {
        let stroke = self.place(name)?;
        self.press_mods(mods)?;
        self.stroke(stroke, true, true)?;
        self.release_mods(mods)
    }

    /// Hold a key for `hold`, with `mods` held around it.
    pub fn hold(&mut self, name: &str, mods: &[Modifier], hold: Duration) -> Result<(), String> {
        let stroke = self.place(name)?;
        self.press_mods(mods)?;
        self.stroke(stroke, true, false)?;
        sleep(hold);
        self.stroke(stroke, false, true)?;
        self.release_mods(mods)
    }

    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        for c in text.chars() {
            let stroke = match c {
                '\n' => self.place("Return")?,
                '\t' => self.place("Tab")?,
                ' ' => self.place("space")?,
                '\r' => continue,
                c => match printable(c) {
                    Some(stroke) => stroke,
                    None => self.place(&unicode_name(c))?,
                },
            };
            self.stroke(stroke, true, true)?;
            sleep(KEY_GAP);
        }
        Ok(())
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        if self.held != 0 {
            self.set_mods(0);
            let _ = self.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_lands_where_a_us_keyboard_has_it() {
        assert_eq!(
            printable('-'),
            Some(Stroke {
                code: 12,
                shift: false
            })
        );
        assert_eq!(
            printable('_'),
            Some(Stroke {
                code: 12,
                shift: true
            })
        );
        assert_eq!(
            printable('A'),
            Some(Stroke {
                code: 30,
                shift: true
            })
        );
        assert_eq!(printable('é'), None);
    }

    #[test]
    fn a_key_is_found_by_character_or_keysym_name() {
        assert_eq!(
            us_key("Return"),
            Some(Stroke {
                code: 28,
                shift: false
            })
        );
        assert_eq!(us_key("minus"), us_key("-"));
        assert_eq!(us_key("question"), us_key("?"));
        assert_eq!(us_key("XF86AudioPlay"), None);
    }

    #[test]
    fn every_code_in_the_keymap_is_bound_once() {
        let map = keymap(&["U00E9".to_string()]);
        let mut codes: Vec<&str> = map
            .lines()
            .filter(|l| l.starts_with("key <I"))
            .map(|l| l.split('>').next().unwrap_or_default())
            .collect();
        let total = codes.len();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), total);
        assert!(map.contains("key <I92> { [ U00E9 ] };"));
        assert!(map.contains("key <I20> { [ U002D, U005F ] };"));
    }
}
