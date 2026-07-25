use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

/// Build the native macOS menu bar. Custom items carry ids like `nav:agenda`
/// or `new:artist`; clicks are forwarded to the frontend via the `menu-action`
/// event (see lib.rs `on_menu_event`).
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;

    // ---- App menu (PiersCRM) ----
    let app_menu = Submenu::new(app, "PiersCRM", true)?;
    app_menu.append_items(&[
        &PredefinedMenuItem::about(app, Some("PiersCRM"), None)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "nav:settings", "Réglages…", true, Some("CmdOrCtrl+,"))?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "check-updates", "Rechercher les mises à jour…", true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::hide(app, None)?,
        &PredefinedMenuItem::hide_others(app, None)?,
        &PredefinedMenuItem::show_all(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::quit(app, None)?,
    ])?;

    // ---- File / Fichier ----
    let file_menu = Submenu::new(app, "Fichier", true)?;
    file_menu.append_items(&[
        &MenuItem::with_id(app, "new:artist", "Nouvel artiste", true, Some("CmdOrCtrl+Shift+A"))?,
        &MenuItem::with_id(app, "new:contact", "Nouveau contact", true, Some("CmdOrCtrl+Shift+C"))?,
        &MenuItem::with_id(app, "new:event", "Nouvel événement", true, Some("CmdOrCtrl+Shift+E"))?,
        &MenuItem::with_id(app, "new:campaign", "Nouvelle campagne", true, Some("CmdOrCtrl+Shift+M"))?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "nav:import", "Importer des données…", true, Some("CmdOrCtrl+I"))?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::close_window(app, None)?,
    ])?;

    // ---- Edit / Édition ----
    let edit_menu = Submenu::new(app, "Édition", true)?;
    edit_menu.append_items(&[
        &PredefinedMenuItem::undo(app, None)?,
        &PredefinedMenuItem::redo(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::cut(app, None)?,
        &PredefinedMenuItem::copy(app, None)?,
        &PredefinedMenuItem::paste(app, None)?,
        &PredefinedMenuItem::select_all(app, None)?,
    ])?;

    // ---- View / Aller à ----
    let view_menu = Submenu::new(app, "Aller à", true)?;
    view_menu.append_items(&[
        &MenuItem::with_id(app, "nav:dashboard", "Tableau de bord", true, Some("CmdOrCtrl+1"))?,
        &MenuItem::with_id(app, "nav:agenda", "Agenda", true, Some("CmdOrCtrl+2"))?,
        &MenuItem::with_id(app, "nav:artists", "Artistes", true, Some("CmdOrCtrl+3"))?,
        &MenuItem::with_id(app, "nav:contacts", "Contacts", true, Some("CmdOrCtrl+4"))?,
        &MenuItem::with_id(app, "nav:emails", "Emails", true, Some("CmdOrCtrl+5"))?,
        &MenuItem::with_id(app, "nav:budget", "Budget", true, Some("CmdOrCtrl+6"))?,
        &MenuItem::with_id(app, "nav:visa", "Visas", true, Some("CmdOrCtrl+7"))?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::fullscreen(app, None)?,
    ])?;

    // ---- Window / Fenêtre ----
    let window_menu = Submenu::new(app, "Fenêtre", true)?;
    window_menu.append_items(&[
        &PredefinedMenuItem::minimize(app, None)?,
        &PredefinedMenuItem::maximize(app, None)?,
    ])?;

    menu.append_items(&[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu])?;
    Ok(menu)
}
