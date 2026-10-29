use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

/// Rebuild the menu bar in the UI language. Called by the frontend at startup
/// and whenever the language changes in Settings (the menu used to stay French
/// even with the app in English).
#[tauri::command]
pub fn set_menu_lang(app: AppHandle, lang: String) -> Result<(), String> {
    let menu = build_lang(&app, &lang).map_err(|e| e.to_string())?;
    app.set_menu(menu).map_err(|e| e.to_string())?;
    Ok(())
}

/// Build the native macOS menu bar. Custom items carry ids like `nav:agenda`
/// or `new:artist`; clicks are forwarded to the frontend via the `menu-action`
/// event (see lib.rs `on_menu_event`).
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    build_lang(app, "fr")
}

pub fn build_lang<R: Runtime>(app: &AppHandle<R>, lang: &str) -> tauri::Result<Menu<R>> {
    let en = lang.starts_with("en");
    let l = |fr: &'static str, english: &'static str| if en { english } else { fr };
    let menu = Menu::new(app)?;

    // ---- App menu (PiersCRM) ----
    let app_menu = Submenu::new(app, "PiersCRM", true)?;
    app_menu.append_items(&[
        &PredefinedMenuItem::about(app, Some("PiersCRM"), None)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "nav:settings", l("Réglages…", "Settings…"), true, Some("CmdOrCtrl+,"))?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "check-updates", l("Rechercher les mises à jour…", "Check for Updates…"), true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::hide(app, None)?,
        &PredefinedMenuItem::hide_others(app, None)?,
        &PredefinedMenuItem::show_all(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::quit(app, None)?,
    ])?;

    // ---- File / Fichier ----
    let file_menu = Submenu::new(app, l("Fichier", "File"), true)?;
    file_menu.append_items(&[
        &MenuItem::with_id(app, "new:artist", l("Nouvel artiste", "New Artist"), true, Some("CmdOrCtrl+Shift+A"))?,
        &MenuItem::with_id(app, "new:contact", l("Nouveau contact", "New Contact"), true, Some("CmdOrCtrl+Shift+C"))?,
        &MenuItem::with_id(app, "new:event", l("Nouvel événement", "New Event"), true, Some("CmdOrCtrl+Shift+E"))?,
        &MenuItem::with_id(app, "new:campaign", l("Nouvelle campagne", "New Campaign"), true, Some("CmdOrCtrl+Shift+M"))?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "nav:import", l("Importer des données…", "Import Data…"), true, Some("CmdOrCtrl+I"))?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::close_window(app, None)?,
    ])?;

    // ---- Edit / Édition ----
    let edit_menu = Submenu::new(app, l("Édition", "Edit"), true)?;
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
    // Same order as the sidebar (NAV_GROUPS in Layout.tsx), one separator per group.
    let go = |id: &str, label: &str, key: &str| {
        MenuItem::with_id(app, id, label, true, Some(&format!("CmdOrCtrl+{key}")))
    };
    let view_menu = Submenu::new(app, l("Aller à", "Go"), true)?;
    view_menu.append_items(&[
        &go("nav:home", l("Accueil", "Home"), "1")?,
        &go("nav:agenda", "Agenda", "2")?,
        &PredefinedMenuItem::separator(app)?,
        &go("nav:artists", l("Artistes", "Artists"), "3")?,
        &go("nav:audience", "Audience", "4")?,
        &PredefinedMenuItem::separator(app)?,
        &go("nav:pipeline", "Pipeline", "5")?,
        &go("nav:discovery", l("Découverte", "Discovery"), "6")?,
        &PredefinedMenuItem::separator(app)?,
        &go("nav:campaigns", l("Campagnes", "Campaigns"), "7")?,
        &go("nav:outbox", l("Boîte d'envoi", "Outbox"), "8")?,
        &PredefinedMenuItem::separator(app)?,
        &go("nav:budget", "Budget", "9")?,
        &go("nav:visa", l("Visas", "Visas"), "0")?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::fullscreen(app, None)?,
    ])?;

    // ---- Window / Fenêtre ----
    let window_menu = Submenu::new(app, l("Fenêtre", "Window"), true)?;
    window_menu.append_items(&[
        &PredefinedMenuItem::minimize(app, None)?,
        &PredefinedMenuItem::maximize(app, None)?,
    ])?;

    menu.append_items(&[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu])?;
    Ok(menu)
}
