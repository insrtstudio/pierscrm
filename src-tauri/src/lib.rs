mod commands;
mod db;
mod menu;
mod models;

use db::DbPool;
use std::fs;
use tauri::{Emitter, Manager};

pub struct AppState {
    pub pool: DbPool,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Native menu bar (macOS) — forwards clicks to the frontend.
            let menu = menu::build(app.handle())?;
            app.set_menu(menu)?;
            app.on_menu_event(|app, event| {
                let id = event.id().0.clone();
                if id == "check-updates" || id.starts_with("nav:") || id.starts_with("new:") {
                    let _ = app.emit("menu-action", id);
                }
            });

            let dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            fs::create_dir_all(&dir).ok();
            let db_path = dir.join("pierscrm.db");
            let pool = db::init_pool(&db_path).expect("failed to init database");
            app.manage(AppState { pool });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // artists
            commands::artists::list_artists,
            commands::artists::get_artist,
            commands::artists::save_artist,
            commands::artists::delete_artist,
            commands::artists::image_to_data_url,
            // contacts
            commands::contacts::list_contacts,
            commands::contacts::get_contact,
            commands::contacts::create_contact,
            commands::contacts::update_contact,
            commands::contacts::update_contact_status,
            commands::contacts::delete_contact,
            commands::contacts::delete_contacts,
            commands::contacts::list_followups,
            commands::contacts::dismiss_followup,
            // import
            commands::import::preview_file,
            commands::import::import_file,
            // email
            commands::email::get_smtp_config,
            commands::email::save_smtp_config,
            commands::email::test_smtp,
            commands::email::send_email,
            commands::email::send_bulk,
            commands::email::list_emails,
            commands::email::apply_opens,
            // campaigns
            commands::campaigns::list_campaigns,
            commands::campaigns::save_campaign,
            commands::campaigns::delete_campaign,
            // events
            commands::events::list_events,
            commands::events::save_event,
            commands::events::delete_event,
            // visa
            commands::visa::list_visa_countries,
            commands::visa::save_visa_country,
            commands::visa::delete_visa_country,
            commands::visa::list_dossiers,
            commands::visa::save_dossier,
            commands::visa::delete_dossier,
            // venue intelligence (J1)
            commands::vi::vi_list_reference_artists,
            commands::vi::vi_save_reference_artist,
            commands::vi::vi_delete_reference_artist,
            commands::vi::vi_list_areas,
            commands::vi::vi_save_area,
            commands::vi::vi_overview,
            // venue intelligence (J2, harvest)
            commands::harvest::vi_resolve_area,
            commands::harvest::vi_resolve_all_areas,
            commands::harvest::vi_start_harvest,
            commands::harvest::vi_resume_run,
            commands::harvest::vi_list_runs,
            commands::harvest::vi_list_venues,
            // venue intelligence (J5, enrichment)
            commands::enrich::vi_start_enrich,
            commands::enrich::vi_venue_detail,
            // templates
            commands::data::list_templates,
            commands::data::save_template,
            commands::data::delete_template,
            // budget
            commands::data::list_budget,
            commands::data::save_budget_item,
            commands::data::delete_budget_item,
            // tasks
            commands::data::list_tasks,
            commands::data::save_task,
            commands::data::delete_task,
            // kpis
            commands::data::list_kpis,
            commands::data::save_kpi,
            commands::data::delete_kpi,
            // settings + dashboard
            commands::data::get_setting,
            commands::data::set_setting,
            commands::data::print_page,
            commands::data::dashboard_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PiersCRM");
}
