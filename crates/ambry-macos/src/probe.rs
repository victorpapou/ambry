//! Looking into AppKit's private view state from a running app, to find out how macOS draws a
//! control. Driven from `target/glass-tune.txt` so experiments need no rebuild.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, MessageReceiver, Sel};
use objc2_foundation::NSString;

const KEYWORDS: [&str; 12] = [
    "glass",
    "toolbar",
    "appearance",
    "style",
    "context",
    "platter",
    "track",
    "background",
    "material",
    "vibran",
    "bezel",
    "selection",
];

/// Methods of `object`'s class chain whose names hint at how it is drawn, with their type
/// encodings, and the values of the ones that read a number.
pub(crate) fn describe(name: &str, object: &AnyObject) -> String {
    let mut out = format!("== {name}: {}\n", object.class().name().to_string_lossy());
    let mut class: Option<&AnyClass> = Some(object.class());
    let mut seen = std::collections::BTreeSet::new();
    while let Some(current) = class {
        if current.name().to_bytes() == b"NSObject" {
            break;
        }
        for method in current.instance_methods().iter() {
            let selector = method.name();
            let text = selector.name().to_string_lossy().into_owned();
            let lower = text.to_lowercase();
            if !KEYWORDS.iter().any(|word| lower.contains(word)) || !seen.insert(text.clone()) {
                continue;
            }
            let returns = method.return_type().to_string_lossy().into_owned();
            out += &format!(
                "  {}  {text} -> {returns}",
                current.name().to_string_lossy()
            );
            if method.arguments_count() == 2 {
                if let Some(value) = unsafe { read(object, selector, &returns) } {
                    out += &format!(" = {value}");
                }
            }
            out += "\n";
        }
        class = current.superclass();
    }
    out
}

/// Calls a zero-argument getter that returns a number or a BOOL.
unsafe fn read(object: &AnyObject, selector: Sel, returns: &str) -> Option<String> {
    // Getters with side effects in their names are not worth the risk.
    let name = selector.name().to_string_lossy();
    if name.starts_with("set") || name.contains("update") || name.contains("begin") {
        return None;
    }
    unsafe {
        Some(match returns {
            "B" => format!(
                "{}",
                object
                    .send_message::<(), objc2::runtime::Bool>(selector, ())
                    .as_bool()
            ),
            "c" => format!("{}", object.send_message::<(), i8>(selector, ())),
            "C" => format!("{}", object.send_message::<(), u8>(selector, ())),
            "q" | "l" => format!("{}", object.send_message::<(), isize>(selector, ())),
            "Q" | "L" => format!("{}", object.send_message::<(), usize>(selector, ())),
            "i" => format!("{}", object.send_message::<(), i32>(selector, ())),
            "I" => format!("{}", object.send_message::<(), u32>(selector, ())),
            "d" => format!("{}", object.send_message::<(), f64>(selector, ())),
            _ => return None,
        })
    }
}

/// Sends a one-argument message whose argument is a number or a BOOL, like
/// `setToolbarAppearance: 1`. Returns what happened.
pub(crate) fn poke(object: &AnyObject, selector: &str, value: f64) -> String {
    let Ok(name) = std::ffi::CString::new(selector) else {
        return format!("bad selector {selector}");
    };
    let sel = Sel::register(&name);
    let Some(method) = object.class().instance_method(sel) else {
        return format!("{selector}: not implemented");
    };
    if method.arguments_count() != 3 {
        return format!(
            "{selector}: takes {} arguments",
            method.arguments_count() - 2
        );
    }
    let kind = method
        .argument_type(2)
        .map(|kind| kind.to_string_lossy().into_owned())
        .unwrap_or_default();
    unsafe {
        match kind.as_str() {
            "B" => object.send_message::<(objc2::runtime::Bool,), ()>(
                sel,
                (objc2::runtime::Bool::new(value != 0.),),
            ),
            "c" => object.send_message::<(i8,), ()>(sel, (value as i8,)),
            "C" => object.send_message::<(u8,), ()>(sel, (value as u8,)),
            "q" | "l" => object.send_message::<(isize,), ()>(sel, (value as isize,)),
            "Q" | "L" => object.send_message::<(usize,), ()>(sel, (value as usize,)),
            "i" => object.send_message::<(i32,), ()>(sel, (value as i32,)),
            "I" => object.send_message::<(u32,), ()>(sel, (value as u32,)),
            "d" => object.send_message::<(f64,), ()>(sel, (value,)),
            other => return format!("{selector}: argument type {other} not supported"),
        }
    }
    format!("{selector} {value}: sent")
}

/// AppKit's own description of `view` and everything under it.
pub(crate) fn subtree(view: &AnyObject) -> String {
    let tree: Option<Retained<NSString>> = unsafe { msg_send![view, _subtreeDescription] };
    tree.map(|tree| tree.to_string()).unwrap_or_default()
}

/// Runs the `poke <target> <selector> <value>` and `dump` lines of `commands` against the
/// named `targets` and reports the results.
pub fn run(targets: &[(String, Retained<AnyObject>)], commands: &str) -> String {
    let mut out = String::new();
    for line in commands.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["poke", target, selector, value] => {
                let Some((_, object)) = targets.iter().find(|(name, _)| name == target) else {
                    out += &format!("no target {target}\n");
                    continue;
                };
                let Ok(value) = value.parse() else {
                    out += &format!("bad value {value}\n");
                    continue;
                };
                out += &poke(object, selector, value);
                out += "\n";
            }
            ["dump"] => {
                for (name, object) in targets {
                    out += &describe(name, object);
                }
                for (name, object) in targets {
                    if name.ends_with("root") {
                        out += &format!("== subtree of {name}\n{}\n", subtree(object));
                    }
                }
            }
            _ => {}
        }
    }
    out
}
