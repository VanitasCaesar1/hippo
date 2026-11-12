//! The macOS menu bar, built from the action table.

use gpui::{Menu, MenuItem};

use crate::actions::{Ctx, MenuName, TABLE};

/// Every menu with its items; an item that cannot run now is disabled.
pub fn build(ctx: &Ctx) -> Vec<Menu> {
    MenuName::ALL
        .iter()
        .map(|menu| {
            let items: Vec<MenuItem> = TABLE
                .iter()
                .filter(|def| def.menu == Some(*menu))
                .filter_map(|def| def.menu_item(ctx))
                .collect();
            Menu::new(menu.title()).items(items)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_with_a_menu_is_in_it_and_the_bar_has_every_menu() {
        let menus = build(&Ctx::default());
        assert_eq!(menus.len(), MenuName::ALL.len());
        let items: usize = menus.iter().map(|m| m.items.len()).sum();
        assert_eq!(items, TABLE.iter().filter(|d| d.menu.is_some()).count());
        assert_eq!(menus[0].name.as_ref(), "Hippocamp");
    }

    #[test]
    fn items_are_disabled_when_their_action_cannot_run() {
        let disabled = |ctx: &Ctx| -> usize {
            build(ctx)
                .iter()
                .flat_map(|m| m.items.iter())
                .filter(|i| i.is_disabled())
                .count()
        };
        let none = disabled(&Ctx::default());
        let more = disabled(&Ctx {
            setup_ready: true,
            depth: 3,
            memory: true,
            filter_has_text: true,
            picker_open: true,
            active_file: true,
            revert_ready: true,
            confirm_open: true,
        });
        assert!(none > more, "{none} vs {more}");
    }
}
