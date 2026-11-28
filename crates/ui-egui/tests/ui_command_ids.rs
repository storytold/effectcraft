//! The engine id list and `UI_COMMANDS` are one list. This fails when they drift.

#[test]
fn ui_command_ids_match_the_menu_table() {
    let ids: Vec<&str> = effectcraft_ui_egui::menus::UI_COMMANDS.iter().map(|command| command.id).collect();
    assert_eq!(ids, effectcraft_engine::ui_ids::ui_command_ids());
}
