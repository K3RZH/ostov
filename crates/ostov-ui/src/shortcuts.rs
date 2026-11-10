//! Сочетания клавиш на любой раскладке.
//!
//! Slint узнаёт Ctrl+C, Ctrl+V и остальные сочетания по символу клавиши. На русской раскладке
//! клавиша «C» даёт «с» кириллицей, и сочетание не срабатывает. Здесь событие клавиатуры
//! перехватывается до Slint: если зажат Ctrl, а клавиша даёт не латинский символ, вместо него
//! в окно уходит латинская буква по месту клавиши на клавиатуре.

use std::cell::Cell;

use slint::platform::WindowEvent;
use slint::winit_030::winit::event::{ElementState, WindowEvent as WinitEvent};
use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use slint::winit_030::{EventResult, WinitWindowAccessor};

/// Включает перевод сочетаний для окна. Вызывается один раз для каждого окна приложения.
/// Окно без winit (тесты) остаётся как есть.
pub fn install(window: &slint::Window) {
    let modifiers = Cell::new(ModifiersState::empty());
    window.on_winit_window_event(move |window, event| match event {
        WinitEvent::ModifiersChanged(changed) => {
            modifiers.set(changed.state());
            EventResult::Propagate
        }
        WinitEvent::KeyboardInput {
            event,
            is_synthetic: false,
            ..
        } => {
            let Some(letter) =
                latin_letter(modifiers.get(), &event.logical_key, event.physical_key)
            else {
                return EventResult::Propagate;
            };
            let text = slint::SharedString::from(letter);
            let translated = match event.state {
                ElementState::Pressed if event.repeat => WindowEvent::KeyPressRepeated { text },
                ElementState::Pressed => WindowEvent::KeyPressed { text },
                ElementState::Released => WindowEvent::KeyReleased { text },
            };
            // Окно могло закрыться во время обработки: тогда событие некому получать.
            let _ = window.dispatch_event_with_result(translated);
            EventResult::PreventDefault
        }
        _ => EventResult::Propagate,
    });
}

/// Латинская буква для сочетания или `None`, если событие надо оставить как есть.
///
/// Заменяется только буква не из ASCII при зажатом Ctrl или Super (Cmd на macOS). Alt вместе
/// с Ctrl не трогаем: на части раскладок это AltGr, которым набирают символы.
fn latin_letter(modifiers: ModifiersState, logical: &Key, physical: PhysicalKey) -> Option<char> {
    let shortcut = modifiers.control_key() || modifiers.super_key();
    if !shortcut || modifiers.alt_key() {
        return None;
    }
    let Key::Character(typed) = logical else {
        return None;
    };
    if typed.is_ascii() {
        return None;
    }
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    let letter = letter_of(code)?;
    Some(if modifiers.shift_key() {
        letter.to_ascii_uppercase()
    } else {
        letter
    })
}

/// Буква, написанная на клавише в раскладке QWERTY.
fn letter_of(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key::Character(text.into())
    }

    #[test]
    fn ctrl_with_cyrillic_letter_becomes_latin() {
        let letter = latin_letter(
            ModifiersState::CONTROL,
            &key("с"),
            PhysicalKey::Code(KeyCode::KeyC),
        );
        assert_eq!(letter, Some('c'));
    }

    #[test]
    fn ctrl_shift_gives_uppercase_letter() {
        let letter = latin_letter(
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            &key("Я"),
            PhysicalKey::Code(KeyCode::KeyZ),
        );
        assert_eq!(letter, Some('Z'));
    }

    #[test]
    fn latin_layout_is_left_alone() {
        // Dvorak и подобные: буква уже латинская, место клавиши значения не имеет.
        let letter = latin_letter(
            ModifiersState::CONTROL,
            &key("j"),
            PhysicalKey::Code(KeyCode::KeyC),
        );
        assert_eq!(letter, None);
    }

    #[test]
    fn typing_without_ctrl_is_left_alone() {
        let letter = latin_letter(
            ModifiersState::empty(),
            &key("с"),
            PhysicalKey::Code(KeyCode::KeyC),
        );
        assert_eq!(letter, None);
    }

    #[test]
    fn alt_gr_combination_is_left_alone() {
        let letter = latin_letter(
            ModifiersState::CONTROL | ModifiersState::ALT,
            &key("ę"),
            PhysicalKey::Code(KeyCode::KeyE),
        );
        assert_eq!(letter, None);
    }

    #[test]
    fn key_without_letter_is_left_alone() {
        let letter = latin_letter(
            ModifiersState::CONTROL,
            &key("ё"),
            PhysicalKey::Code(KeyCode::Backquote),
        );
        assert_eq!(letter, None);
    }
}
