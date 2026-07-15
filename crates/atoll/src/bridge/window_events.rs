use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::keyboard::ModifiersState;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use crate::browser::{Browser, KeyboardFocus};
use crate::ui::AppWindow;

/// Routes raw window events: keyboard input to shortcuts or the page, scale changes to the engine.
pub fn install(window: &AppWindow, browser: &Rc<Browser>) {
    let modifiers = Rc::new(Cell::new(ModifiersState::empty()));
    let weak_browser = Rc::downgrade(browser);
    let weak_window = window.as_weak();
    window.window().on_winit_window_event(move |_, event| {
        let (Some(browser), Some(window)) = (weak_browser.upgrade(), weak_window.upgrade()) else {
            return EventResult::Propagate;
        };
        let focus = KeyboardFocus {
            page: window.get_web_focused(),
            dev_tools: window.get_dev_tools_focused(),
            modal: window.get_modal_open(),
        };
        match event {
            WindowEvent::ModifiersChanged(state) => {
                modifiers.set(state.state());
                EventResult::Propagate
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => browser.handle_key(event, modifiers.get(), focus),
            WindowEvent::Ime(ime) => browser.handle_ime(ime, focus),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                browser.set_scale_factor(*scale_factor as f32);
                EventResult::Propagate
            }
            WindowEvent::Focused(false) => {
                modifiers.set(ModifiersState::empty());
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        }
    });
}
