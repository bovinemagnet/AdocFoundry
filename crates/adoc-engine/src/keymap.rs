//! The keys commands have: each command's own (`CommandSpec::default_binding`), and over them
//! the user settings file's `[keymap]`. The engine decides which command has which keys; a front
//! end binds them.

use std::collections::BTreeMap;

use crate::{CommandId, CommandRegistry, Engine, Settings};

/// What a key may be called, beside a letter, digit, sign or function key.
const NAMED: [&str; 15] = [
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
];
const MODIFIERS: [&str; 4] = ["cmd", "ctrl", "alt", "shift"];

/// Keys in the notation of [`crate::CommandSpec::default_binding`]: strokes pressed one after
/// another, each its modifiers (`cmd`, `ctrl`, `alt` and `shift`, in that order) and a key.
/// `cmd` is the platform's primary modifier: Command on macOS, Control elsewhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Keys(String);

impl Keys {
    /// Reads `text`, whose modifiers may come in any order and in capitals.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let strokes = text
            .split_whitespace()
            .map(stroke)
            .collect::<Result<Vec<_>, _>>()?;
        if strokes.is_empty() {
            return Err("expected keys such as \"cmd-shift-x\"");
        }
        Ok(Self(strokes.join(" ")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the first stroke is one that types, moves the caret or answers a dialog.
    fn begins_by_typing(&self) -> bool {
        let first = self.0.split(' ').next().unwrap_or_default();
        let (modifiers, key) = split_stroke(first);
        !is_function_key(key) && modifiers.iter().all(|modifier| *modifier == "shift")
    }
}

/// The modifiers and key of one stroke, whose key may be the hyphen itself (`cmd--`).
fn split_stroke(stroke: &str) -> (Vec<&str>, &str) {
    let modifiers = match stroke.strip_suffix("--") {
        Some(modifiers) => Some((modifiers, "-")),
        None => stroke.rsplit_once('-').filter(|_| stroke != "-"),
    };
    match modifiers {
        Some((modifiers, key)) => (modifiers.split('-').collect(), key),
        None => (Vec::new(), stroke),
    }
}

/// `f1` to `f24`.
fn is_function_key(key: &str) -> bool {
    (1..=24).any(|number| key == format!("f{number}"))
}

fn stroke(text: &str) -> Result<String, &'static str> {
    let text = text.to_lowercase();
    let (modifiers, key) = split_stroke(&text);
    if modifiers
        .iter()
        .any(|modifier| !MODIFIERS.contains(modifier))
    {
        return Err("expected the modifiers cmd, ctrl, alt and shift");
    }
    let mut chars = key.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(only), None) => !only.is_whitespace() && !only.is_control(),
        _ => false,
    };
    if !(single || is_function_key(key) || NAMED.contains(&key)) {
        return Err("expected a key: a letter, digit or sign, f1 to f24, or a name such as \"enter\" or \"pageup\"");
    }
    let mut stroke = String::new();
    for modifier in MODIFIERS {
        if modifiers.contains(&modifier) {
            stroke.push_str(modifier);
            stroke.push('-');
        }
    }
    stroke.push_str(key);
    Ok(stroke)
}

/// Which keys each command has; see [`Engine::keymap`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Keymap {
    keys: BTreeMap<CommandId, String>,
    /// The commands whose keys, or none, the settings give.
    custom: Vec<CommandId>,
    problems: Vec<String>,
}

impl Keymap {
    /// The keys of command `id`.
    #[must_use]
    pub fn keys(&self, id: CommandId) -> Option<&str> {
        self.keys.get(&id).map(String::as_str)
    }

    /// Whether the settings give command `id` its keys, or take them away.
    #[must_use]
    pub fn is_custom(&self, id: CommandId) -> bool {
        self.custom.contains(&id)
    }

    /// What of `[keymap]` could not be followed, or may surprise.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    fn resolve(commands: &CommandRegistry, settings: &Settings) -> Self {
        let mut keymap = Self::default();
        let warn =
            |name: &str, problem: &str| settings.user_warning(&format!("keymap.{name}"), problem);
        // Those the settings give, the first of two that want the same keys having them.
        let mut given: BTreeMap<String, CommandId> = BTreeMap::new();
        for (name, keys) in &settings.keymap {
            let Some(spec) = commands.all().find(|spec| spec.id.as_str() == name) else {
                keymap.problems.push(warn(name, "there is no such command"));
                continue;
            };
            let Some(keys) = keys else {
                keymap.custom.push(spec.id);
                continue;
            };
            if keys.begins_by_typing() && spec.default_binding != Some(keys.as_str()) {
                keymap.problems.push(warn(
                    name,
                    "begins with a key that types or moves; use cmd, ctrl or alt with it, or a \
                     function key",
                ));
                continue;
            }
            if let Some(other) = given.get(&comparable(keys.as_str())) {
                let problem = format!("`{}` is given to `{}` too", keys.as_str(), other.as_str());
                keymap.problems.push(warn(name, &problem));
                continue;
            }
            given.insert(comparable(keys.as_str()), spec.id);
            keymap.custom.push(spec.id);
            keymap.keys.insert(spec.id, keys.as_str().to_owned());
        }
        for spec in commands.all() {
            if let Some(keys) = spec
                .default_binding
                .filter(|keys| !given.contains_key(&comparable(keys)))
                .filter(|_| !keymap.is_custom(spec.id))
            {
                keymap.keys.insert(spec.id, keys.to_owned());
            }
        }
        // Keys that begin others'.
        for (given, id) in &given {
            let given_name = id.as_str();
            for (other, keys) in &keymap.keys {
                if other == id {
                    continue;
                }
                let keys_compared = comparable(keys);
                let (begins, begun) = if keys_compared.starts_with(&format!("{given} ")) {
                    (given_name, other.as_str())
                } else if given.starts_with(&format!("{keys_compared} ")) {
                    (other.as_str(), given_name)
                } else {
                    continue;
                };
                let problem = format!(
                    "the keys of `{begins}` also begin those of `{begun}`, so one may not work"
                );
                keymap.problems.push(warn(given_name, &problem));
                break;
            }
        }
        keymap
    }
}

/// `keys` as they compare with others': away from macOS `cmd` and `ctrl` are one key.
fn comparable(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        return keys.to_owned();
    }
    keys.split(' ')
        .map(|stroke| {
            let (modifiers, key) = split_stroke(stroke);
            let mut compared: Vec<&str> = modifiers
                .into_iter()
                .map(|modifier| if modifier == "cmd" { "ctrl" } else { modifier })
                .collect();
            compared.dedup();
            compared.push(key);
            compared.join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Engine {
    /// Which keys each command has: its own, and over them the settings' `[keymap]`.
    #[must_use]
    pub fn keymap(&self) -> Keymap {
        Keymap::resolve(self.commands(), &self.settings)
    }
}
