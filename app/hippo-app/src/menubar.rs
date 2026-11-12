//! The menu-bar item and open at login.
//!
//! The pinned GPUI has no status item, so this makes one with `objc2-app-kit`
//! (the crates GPUI's macOS platform links). Everything here runs on the
//! main thread: the app calls it from GPUI's foreground tasks, which run
//! there, and each call takes a [`MainThreadMarker`]. A menu click sends its
//! [`Act`] down a channel that a foreground task reads, so no AppKit
//! callback ever re-enters GPUI.

use std::cell::RefCell;

use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSCellImagePosition, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSError, NSSize, NSString};

use crate::reach::{Act, Menu};

/// The hippo mark, drawn as a template image (the menu bar tints it).
const MARK: &[u8] = include_bytes!("../../../brand/hippo-mark.svg");

struct Ivars {
    sender: UnboundedSender<Act>,
    /// The act of each enabled item, by its tag.
    acts: RefCell<Vec<Act>>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements and `MenuTarget`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(clicked:))]
        fn clicked(&self, sender: &NSMenuItem) {
            let tag = sender.tag();
            let act = usize::try_from(tag)
                .ok()
                .and_then(|i| self.ivars().acts.borrow().get(i).cloned());
            if let Some(act) = act {
                self.ivars().sender.unbounded_send(act).ok();
            }
        }
    }
);

impl MenuTarget {
    fn new(sender: UnboundedSender<Act>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            sender,
            acts: RefCell::new(Vec::new()),
        });
        // SAFETY: `NSObject`'s `init` is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// The status item: shown while this lives.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    target: Retained<MenuTarget>,
    shown: Option<Menu>,
}

impl StatusItem {
    /// Puts the item in the menu bar. `None` off the main thread.
    pub fn new(sender: UnboundedSender<Act>) -> Option<StatusItem> {
        let mtm = MainThreadMarker::new()?;
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            let data = NSData::with_bytes(MARK);
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                // 473 by 387 points in the source.
                image.setSize(NSSize::new(19.0, 15.5));
                image.setTemplate(true);
                button.setImage(Some(&image));
                button.setImagePosition(NSCellImagePosition::ImageLeft);
            } else {
                button.setTitle(&NSString::from_str("hippo"));
            }
            button.setToolTip(Some(&NSString::from_str("Hippo")));
        }
        let target = MenuTarget::new(sender, mtm);
        let mut s = StatusItem {
            item,
            target,
            shown: None,
        };
        s.update(&Menu::default());
        Some(s)
    }

    /// Shows `menu`: the badge beside the glyph and the menu, rebuilt only
    /// when something in it changed.
    pub fn update(&mut self, menu: &Menu) {
        if self.shown.as_ref() == Some(menu) {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if let Some(button) = self.item.button(mtm) {
            let title = if menu.badge.is_empty() {
                String::new()
            } else {
                format!(" {}", menu.badge)
            };
            button.setTitle(&NSString::from_str(&title));
        }
        let ns = NSMenu::new(mtm);
        ns.setAutoenablesItems(false);
        let mut acts: Vec<Act> = Vec::new();
        let subtitles =
            AnyClass::get(c"NSMenuItem").is_some_and(|c| c.responds_to(sel!(setSubtitle:)));
        let header = |text: &str| {
            let it = item(mtm, text, None);
            it.setEnabled(false);
            ns.addItem(&it);
        };
        let mut add = |title: &str, sub: Option<&str>, act: Act, key: &str| {
            let it = if let Some(s) = sub.filter(|_| !subtitles) {
                item(mtm, &format!("{title} — {s}"), Some(key))
            } else {
                item(mtm, title, Some(key))
            };
            if let Some(s) = sub.filter(|_| subtitles) {
                it.setSubtitle(Some(&NSString::from_str(s)));
            }
            // SAFETY: the target is retained by `self` for as long as the
            // menu that holds the item.
            unsafe { it.setTarget(Some(&*self.target as &AnyObject)) };
            it.setTag(acts.len() as isize);
            acts.push(act);
            ns.addItem(&it);
        };

        header(&menu.needs_head);
        for row in &menu.needs {
            add(&row.title, Some(&row.reason), row.act.clone(), "");
        }
        if let Some(n) = &menu.nothing {
            header(n);
        }
        // What live agents are doing, the Inbox, a row per profile.
        if let Some(h) = &menu.live_head {
            ns.addItem(&NSMenuItem::separatorItem(mtm));
            header(h);
            for row in &menu.live {
                add(&row.title, Some(&row.reason), row.act.clone(), "");
            }
        }
        ns.addItem(&NSMenuItem::separatorItem(mtm));
        if let Some(h) = &menu.headroom_head {
            header(h);
            // Enabled with no action: each reads as text, not as a dimmed
            // heading, and a click does nothing. Each has a bar of
            // what is left, drawn as coloured text (no custom views).
            for (i, line) in menu.headroom.iter().enumerate() {
                let it = item(mtm, line, None);
                if let Some(&(left, guess)) = menu.bars.get(i) {
                    bar_title(&it, line, f64::from(left) / 100.0, guess);
                }
                ns.addItem(&it);
            }
            ns.addItem(&NSMenuItem::separatorItem(mtm));
        }
        if let Some(i) = &menu.inbox {
            add(i, None, Act::Inbox, "");
        }
        add("Brief a new agent…", None, Act::Brief, "");
        add("Open Hippo", None, Act::OpenHippo, "");
        ns.addItem(&NSMenuItem::separatorItem(mtm));
        add("Quit Hippo", None, Act::Quit, "q");

        *self.target.ivars().acts.borrow_mut() = acts;
        self.item.setMenu(Some(&ns));
        self.shown = Some(menu.clone());
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

/// `NSRange`, for `addAttribute:value:range:` through the runtime.
#[repr(C)]
#[derive(Clone, Copy)]
struct Range {
    location: usize,
    length: usize,
}

// SAFETY: the layout and encoding of Foundation's `NSRange`.
unsafe impl objc2::Encode for Range {
    const ENCODING: objc2::Encoding =
        objc2::Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
}

/// How many cells a headroom bar has.
const BAR: usize = 10;

/// A headroom row as `name ⇥ ━━━━━━━━━━  97% left  use next`: the bar's
/// filled cells in the ring's colour (the secondary label colour; orange
/// under 25 %, red under 10 %; grey when inferred), the rest in the tertiary label colour, the
/// bar column on one tab stop. Every class and selector is checked first;
/// when one is missing the row keeps its plain title.
fn bar_title(it: &NSMenuItem, line: &str, left: f64, guess: bool) {
    let (name, rest) = match line.split_once("   ") {
        Some((n, r)) => (n, r.replace("   ", "  ")),
        None => (line, String::new()),
    };
    let filled = ((left.clamp(0.0, 1.0) * BAR as f64).round() as usize).min(BAR);
    let text = format!(
        "{name}\t{}{}  {rest}",
        "━".repeat(filled),
        "━".repeat(BAR - filled)
    );
    let units = |s: &str| s.encode_utf16().count();
    let bar_at = units(name) + 1;
    let (Some(mas), Some(color), Some(track), Some(para)) = (
        AnyClass::get(c"NSMutableAttributedString"),
        AnyClass::get(c"NSColor"),
        AnyClass::get(c"NSMutableParagraphStyle"),
        AnyClass::get(c"NSTextTab"),
    ) else {
        return;
    };
    let colour = |left: f64, guess: bool| -> Option<Retained<AnyObject>> {
        // SAFETY: class methods of NSColor returning autoreleased colours,
        // each checked before it is sent.
        unsafe {
            if guess || left <= 0.0 {
                color
                    .class_method(sel!(tertiaryLabelColor))
                    .map(|_| msg_send![color, tertiaryLabelColor])
            } else if left < 0.10 {
                color
                    .class_method(sel!(systemRedColor))
                    .map(|_| msg_send![color, systemRedColor])
            } else if left < 0.25 {
                color
                    .class_method(sel!(systemOrangeColor))
                    .map(|_| msg_send![color, systemOrangeColor])
            } else {
                // Healthy headroom is not a state.
                color
                    .class_method(sel!(secondaryLabelColor))
                    .map(|_| msg_send![color, secondaryLabelColor])
            }
        }
    };
    let (Some(fill), Some(rest_color)) = (colour(left, guess), colour(0.0, true)) else {
        return;
    };
    // `initWithString:` is not reported on the class cluster's facade
    // (its placeholder implements it); it is Foundation's own initialiser.
    if !mas.responds_to(sel!(addAttribute:value:range:))
        || !track.responds_to(sel!(setTabStops:))
        || !para.responds_to(sel!(initWithTextAlignment:location:options:))
        || !it.respondsToSelector(sel!(setAttributedTitle:))
    {
        return;
    }
    // SAFETY: the classes and selectors were checked above; every object
    // is retained for the length of the call, and the ranges are in UTF-16
    // units inside `text`.
    unsafe {
        let ns = NSString::from_str(&text);
        let alloc: objc2::rc::Allocated<AnyObject> = msg_send![mas, alloc];
        let s: Retained<AnyObject> = msg_send![alloc, initWithString: &*ns];
        let key_color = NSString::from_str("NSColor");
        let key_para = NSString::from_str("NSParagraphStyle");
        let r_fill = Range {
            location: bar_at,
            length: filled,
        };
        let r_rest = Range {
            location: bar_at + filled,
            length: BAR - filled,
        };
        if filled > 0 {
            let _: () = msg_send![&*s, addAttribute: &*key_color, value: &*fill, range: r_fill];
        }
        if filled < BAR {
            let _: () =
                msg_send![&*s, addAttribute: &*key_color, value: &*rest_color, range: r_rest];
        }
        let style: Retained<AnyObject> = msg_send![track, new];
        let dict_class = AnyClass::get(c"NSDictionary");
        let array_class = AnyClass::get(c"NSArray");
        if let (Some(dc), Some(ac)) = (dict_class, array_class) {
            let empty: Retained<AnyObject> = msg_send![dc, dictionary];
            let talloc: objc2::rc::Allocated<AnyObject> = msg_send![para, alloc];
            let tab: Retained<AnyObject> = msg_send![
                talloc,
                initWithTextAlignment: 0isize,
                location: 104.0f64,
                options: &*empty
            ];
            let tabs: Retained<AnyObject> = msg_send![ac, arrayWithObject: &*tab];
            let _: () = msg_send![&*style, setTabStops: &*tabs];
            let all = Range {
                location: 0,
                length: units(&text),
            };
            let _: () = msg_send![&*s, addAttribute: &*key_para, value: &*style, range: all];
        }
        let _: () = msg_send![it, setAttributedTitle: &*s];
    }
}

fn item(mtm: MainThreadMarker, title: &str, key: Option<&str>) -> Retained<NSMenuItem> {
    let action = key.map(|_| sel!(clicked:));
    // SAFETY: `clicked:` is the target's method, taking the item.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key.unwrap_or("")),
        )
    }
}

/// Open at login, through `SMAppService.mainAppService`
/// (ServiceManagement, macOS 13 and later), called through the Objective-C
/// runtime so no binding crate is needed.
pub mod login {
    use super::*;

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}

    /// `SMAppServiceStatus`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Status {
        NotRegistered,
        Enabled,
        RequiresApproval,
        NotFound,
    }

    fn service() -> Option<Retained<AnyObject>> {
        let cls = AnyClass::get(c"SMAppService")?;
        // SAFETY: `+[SMAppService mainAppService]` returns the app's service.
        unsafe { msg_send![cls, mainAppService] }
    }

    /// The service's status; `None` where the class is missing.
    pub fn status() -> Option<Status> {
        let svc = service()?;
        // SAFETY: `status` is an `NSInteger` property.
        let st: isize = unsafe { msg_send![&*svc, status] };
        Some(match st {
            0 => Status::NotRegistered,
            1 => Status::Enabled,
            2 => Status::RequiresApproval,
            _ => Status::NotFound,
        })
    }

    /// Registers (on) or unregisters (off) the app as a login item.
    pub fn set(on: bool) -> Result<(), String> {
        let svc = service().ok_or("ServiceManagement is not available")?;
        let mut err: *mut NSError = std::ptr::null_mut();
        // SAFETY: both methods take an `NSError **` and return `BOOL`.
        let ok: Bool = unsafe {
            if on {
                msg_send![&*svc, registerAndReturnError: &mut err]
            } else {
                msg_send![&*svc, unregisterAndReturnError: &mut err]
            }
        };
        if ok.as_bool() {
            return Ok(());
        }
        // SAFETY: a failed call leaves a valid `NSError` or null.
        Err(unsafe { err.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_else(|| "the system refused".into()))
    }
}
