use muda::{
    Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use winit::event_loop::EventLoopProxy;

use crate::{
    action::{Action, KeyBindings},
    app::UserEvent,
};

pub struct NativeMenu {
    _menu: Menu,
}

pub fn install(proxy: EventLoopProxy<UserEvent>, bindings: &KeyBindings) -> NativeMenu {
    let menu = Menu::new();
    let app = Submenu::new("Grin", true);
    app.append(&PredefinedMenuItem::about(Some("About Grin"), None))
        .expect("menu");
    app.append(&PredefinedMenuItem::separator()).expect("menu");
    app.append(&PredefinedMenuItem::hide(Some("Hide Grin")))
        .expect("menu");
    app.append(&PredefinedMenuItem::hide_others(Some("Hide Others")))
        .expect("menu");
    app.append(&PredefinedMenuItem::show_all(Some("Show All")))
        .expect("menu");
    app.append(&PredefinedMenuItem::separator()).expect("menu");
    app.append(&PredefinedMenuItem::quit(Some("Quit Grin")))
        .expect("menu");
    let file = Submenu::new("File", true);

    append(&file, "New Tab", Action::NewTab, bindings);
    append(&file, "Duplicate Tab", Action::DuplicateTab, bindings);
    append(&file, "Restore Closed Tab", Action::RestoreTab, bindings);

    file.append(&PredefinedMenuItem::separator()).expect("menu");

    append(&file, "Close", Action::Close, bindings);
    let edit = Submenu::new("Edit", true);

    append(&edit, "Copy", Action::Copy, bindings);
    append(&edit, "Paste", Action::Paste, bindings);

    edit.append(&PredefinedMenuItem::separator()).expect("menu");

    append(&edit, "Search", Action::Search, bindings);
    let terminal = Submenu::new("Terminal", true);
    append(&terminal, "Split Right", Action::SplitVertical, bindings);
    append(&terminal, "Split Down", Action::SplitHorizontal, bindings);

    terminal
        .append(&PredefinedMenuItem::separator())
        .expect("menu");

    append(&terminal, "Zoom Pane", Action::TogglePaneZoom, bindings);

    terminal
        .append(&PredefinedMenuItem::separator())
        .expect("menu");

    append(&terminal, "Focus Left", Action::FocusLeft, bindings);
    append(&terminal, "Focus Right", Action::FocusRight, bindings);
    append(&terminal, "Focus Up", Action::FocusUp, bindings);
    append(&terminal, "Focus Down", Action::FocusDown, bindings);
    let view = Submenu::new("View", true);
    append(
        &view,
        "Command Palette",
        Action::ToggleCommandPalette,
        bindings,
    );
    let window = Submenu::new("Window", true);
    append(&window, "Next Tab", Action::NextTab, bindings);
    append(&window, "Previous Tab", Action::PreviousTab, bindings);
    let help = Submenu::new("Help", true);
    menu.append_items(&[&app, &file, &edit, &terminal, &view, &window, &help])
        .expect("menu");
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(action) = Action::parse(&event.id.0) {
            let _ = proxy.send_event(UserEvent::MenuAction(action));
        }
    }));
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    help.set_as_help_menu_for_nsapp();
    NativeMenu { _menu: menu }
}

fn append(menu: &Submenu, title: &str, action: Action, bindings: &KeyBindings) {
    menu.append(&MenuItem::with_id(
        action.to_string(),
        title,
        true,
        bindings.shortcut(action).and_then(accelerator),
    ))
    .expect("menu");
}

fn accelerator(chord: crate::action::KeyChord) -> Option<Accelerator> {
    let code = match chord.key.as_str() {
        "t" => Code::KeyT,
        "w" => Code::KeyW,
        "p" => Code::KeyP,
        "c" => Code::KeyC,
        "v" => Code::KeyV,
        "f" => Code::KeyF,
        "d" => Code::KeyD,
        "r" => Code::KeyR,
        "enter" => Code::Enter,
        "left" => Code::ArrowLeft,
        "right" => Code::ArrowRight,
        "up" => Code::ArrowUp,
        "down" => Code::ArrowDown,
        _ => return None,
    };
    let mut modifiers = Modifiers::empty();
    if chord.command {
        modifiers |= Modifiers::SUPER;
    }
    if chord.control {
        modifiers |= Modifiers::CONTROL;
    }
    if chord.alt {
        modifiers |= Modifiers::ALT;
    }
    if chord.shift {
        modifiers |= Modifiers::SHIFT;
    }
    Some(Accelerator::new(Some(modifiers), code))
}
