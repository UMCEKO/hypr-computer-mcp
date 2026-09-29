//! Buttons and the wheel through a wlr virtual pointer; the cursor itself is placed with `hyprctl`.

use std::time::Instant;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// What one wheel notch moves, in the units compositors expect alongside a discrete step.
const NOTCH: f64 = 15.0;

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

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

pub struct Pointer {
    queue: EventQueue<State>,
    pointer: ZwlrVirtualPointerV1,
    discrete: bool,
    epoch: Instant,
}

#[derive(Clone, Copy)]
pub enum Wheel {
    Up,
    Down,
    Left,
    Right,
}

impl Pointer {
    pub fn connect() -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
        let handle = queue.handle();
        let manager: ZwlrVirtualPointerManagerV1 = globals
            .bind(&handle, 1..=2, ())
            .map_err(|e| format!("the compositor offers no virtual pointer: {e}"))?;
        let pointer = manager.create_virtual_pointer(None, &handle, ());
        queue
            .roundtrip(&mut State)
            .map_err(|e| format!("wayland: {e}"))?;
        Ok(Self {
            discrete: wayland_client::Proxy::version(&manager) >= 2,
            queue,
            pointer,
            epoch: Instant::now(),
        })
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

    /// A net-zero wiggle after the cursor was warped, since a button carries no position and a client clicks where it last saw motion.
    pub fn nudge(&mut self) -> Result<(), String> {
        for dx in [1.0, -1.0] {
            self.pointer.motion(self.now(), dx, 0.0);
            self.pointer.frame();
        }
        self.flush()
    }

    pub fn button(&mut self, button: u32, pressed: bool) -> Result<(), String> {
        let state = match pressed {
            true => wl_pointer::ButtonState::Pressed,
            false => wl_pointer::ButtonState::Released,
        };
        self.pointer.button(self.now(), button, state);
        self.pointer.frame();
        self.flush()
    }

    pub fn scroll(&mut self, wheel: Wheel, notches: u32) -> Result<(), String> {
        let (axis, sign) = match wheel {
            Wheel::Up => (wl_pointer::Axis::VerticalScroll, -1),
            Wheel::Down => (wl_pointer::Axis::VerticalScroll, 1),
            Wheel::Left => (wl_pointer::Axis::HorizontalScroll, -1),
            Wheel::Right => (wl_pointer::Axis::HorizontalScroll, 1),
        };
        for _ in 0..notches {
            let time = self.now();
            self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
            match self.discrete {
                true => self
                    .pointer
                    .axis_discrete(time, axis, NOTCH * f64::from(sign), sign),
                false => self.pointer.axis(time, axis, NOTCH * f64::from(sign)),
            }
            self.pointer.frame();
            self.flush()?;
        }
        Ok(())
    }
}
