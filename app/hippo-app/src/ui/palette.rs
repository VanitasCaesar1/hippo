//! The command palette's rows: every action with a palette name, its binding
//! at the right, and a dimmed reason when it cannot run now.

use super::picker::PickerItem;
use crate::actions::{Ctx, Def, palette_order};

/// The definitions and their picker rows, in the palette's order. Row `i` of
/// the picker is `defs[i]`.
pub fn rows(ctx: &Ctx, ran: &[&str]) -> (Vec<&'static Def>, Vec<PickerItem>) {
    let defs = palette_order(ran);
    let items = defs
        .iter()
        .map(|def| {
            let mut item = PickerItem::new(def.palette.unwrap_or(def.id));
            let keys = def.key_label();
            if !keys.is_empty() {
                item.keys = Some(keys);
            }
            item.unavailable = def.check(ctx).err();
            item
        })
        .collect();
    (defs, items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::TABLE;

    #[test]
    fn the_palette_lists_every_menu_action_with_its_binding_and_its_reason() {
        let (defs, items) = rows(&Ctx::default(), &[]);
        assert_eq!(defs.len(), items.len());
        for def in TABLE.iter().filter(|d| d.menu.is_some()) {
            assert!(defs.iter().any(|d| d.id == def.id), "{} missing", def.id);
        }
        let quit = items.iter().find(|i| i.text == "Hippo: Quit").unwrap();
        assert_eq!(quit.keys.as_deref(), Some("⌘Q"));
        assert_eq!(quit.unavailable, None);
        let back = items.iter().find(|i| i.text == "View: Back").unwrap();
        assert_eq!(
            back.unavailable.as_deref(),
            Some("nothing is open to go back from")
        );
        assert!(
            back.accessible_label()
                .contains("unavailable: nothing is open to go back from")
        );
    }

    #[test]
    fn actions_run_this_session_come_first() {
        let (_, items) = rows(&Ctx::default(), &["Reload"]);
        assert_eq!(items[0].text, "View: Reload");
    }
}
