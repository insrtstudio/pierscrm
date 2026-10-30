mod commands;
mod crm_links;
mod db;
mod menu;
mod models;
mod secrets;

use db::DbPool;
use std::collections::HashSet;
use std::fs;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

/// Run ids the user has asked to stop. Workers check this each loop and exit.
pub type CancelSet = Arc<Mutex<HashSet<i64>>>;

pub struct AppState {
    pub pool: DbPool,
    pub cancels: CancelSet,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Window placement. `center` + `maximized` in tauri.conf.json raced on macOS
            // (the window could land offset from the screen's center), so the window
            // starts hidden and we fit it to the work area of the screen it opens on
            // (menu bar and Dock excluded) before showing it.
            if let Some(win) = app.get_webview_window("main") {
                let monitor = win
                    .current_monitor()
                    .ok()
                    .flatten()
                    .or_else(|| win.primary_monitor().ok().flatten());
                if let Some(m) = monitor {
                    let area = m.work_area();
                    let _ = win.set_size(area.size);
                    let _ = win.set_position(area.position);
                } else {
                    let _ = win.center();
                }
                let _ = win.show();
                let _ = win.set_focus();
            }

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
            // Move secrets still in clear in `settings` to the Keychain (verified
            // before the DB copy is blanked; on failure the DB copy is kept).
            if let Ok(conn) = pool.get() {
                secrets::migrate(&conn);
            }
            let pool_for_pulse = pool.clone();
            let pool_for_residency = pool.clone();
            app.manage(AppState {
                pool,
                cancels: Arc::new(Mutex::new(HashSet::new())),
            });
            // Daily Pulse snapshot at launch (only if none was taken today).
            commands::pulse::auto_snapshot_on_launch(pool_for_pulse, app.handle().clone());
            // Residency autopilot: sends validated sequences, reads replies,
            // polls EPK activity, writes the daily report (see commands/residency).
            commands::residency::engine::start(pool_for_residency, app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // artists
            commands::artists::list_artists,
            commands::artists::get_artist,
            commands::artists::get_artist_avatar,
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
            // module links (audit items 5, 9, 10, 16, 17)
            commands::contacts::count_contacts,
            commands::contacts::update_contacts_status,
            commands::contacts::assign_contacts_artist,
            commands::contacts::list_organisations,
            commands::contacts::save_organisation,
            commands::contacts::delete_organisation,
            commands::contacts::count_contacts_without_artist,
            commands::contacts::assign_unassigned_contacts_artist,
            commands::organisations::list_organisation_proposals,
            commands::organisations::accept_organisation_proposal,
            commands::organisations::dismiss_organisation_proposal,
            commands::organisations::set_contacts_organisation,
            commands::organisations::get_organisation_detail,
            commands::visa::list_event_visas,
            commands::contacts::list_followups,
            commands::contacts::dismiss_followup,
            commands::contacts::restore_followup,
            commands::contacts::list_dismissed_followups,
            // import
            commands::import::preview_file,
            commands::import::import_file,
            // email
            commands::email::get_smtp_config,
            commands::email::save_smtp_config,
            commands::email::test_smtp,
            commands::email::send_email,
            commands::email::send_bulk,
            commands::email::cancel_bulk_send,
            commands::funnel::funnel_report,
            commands::funnel::funnel_keep_variant,
            commands::funnel::funnel_digest,
            commands::email::list_emails,
            commands::email::apply_opens,
            // campaigns
            commands::campaigns::list_campaigns,
            commands::campaigns::save_campaign,
            commands::campaigns::delete_campaign,
            commands::campaigns::list_campaign_recipients,
            commands::campaigns::add_to_campaign,
            commands::campaigns::remove_from_campaign,
            commands::campaigns::list_campaign_audience,
            // outbox guardrails + reply detection
            commands::outbox::list_suppressions,
            commands::outbox::add_suppression,
            commands::outbox::remove_suppression,
            commands::outbox::send_quota,
            commands::outbox::list_emails_page,
            commands::outbox::sync_replies,
            commands::outbox::reply_sync_status,
            commands::deliverability::deliverability_status,
            commands::deliverability::deliverability_check_dns,
            commands::deliverability::deliverability_save_budget,
            commands::deliverability::deliverability_save_thresholds,
            commands::deliverability::deliverability_resume,
            // residency campaigns (autopilot)
            commands::residency::commands::residency_presets,
            commands::residency::commands::residency_create,
            commands::residency::commands::residency_get,
            commands::residency::commands::residency_save,
            commands::residency::commands::residency_set_autopilot,
            commands::residency::commands::residency_build_target,
            commands::residency::commands::residency_add_targets,
            commands::residency::commands::residency_remove_target,
            commands::residency::commands::residency_board,
            commands::residency::commands::residency_generate,
            commands::residency::commands::residency_messages,
            commands::residency::commands::residency_save_message,
            commands::residency::commands::residency_set_message_status,
            commands::residency::commands::residency_validate_all,
            commands::residency::commands::residency_reports,
            commands::residency::commands::residency_report_now,
            commands::residency::commands::residency_log,
            commands::residency::commands::residency_actions,
            commands::residency::commands::residency_mark_handled,
            commands::residency::commands::residency_press_kits,
            commands::residency::commands::insrt_contact_activity,
            commands::residency::commands::insrt_sync_activity,
            commands::insrt_gigs::insrt_gig_get,
            commands::insrt_gigs::insrt_gig_set,
            commands::residency::commands::ai_cost_month,
            // réponses à traiter (reply desk)
            commands::reply_desk::desk_list,
            commands::reply_desk::desk_count,
            commands::reply_desk::desk_refresh,
            commands::reply_desk::desk_save,
            commands::reply_desk::desk_send,
            commands::reply_desk::desk_ignore,
            commands::reply_desk::desk_snooze,
            commands::reply_desk::desk_redraft,
            // events
            commands::events::list_events,
            commands::events::save_event,
            commands::events::delete_event,
            // tour routing, rebook loop, played at (commands/routing.rs)
            commands::routing::routing_tour_plan,
            commands::routing::routing_tour_actions,
            commands::routing::routing_pitch_draft,
            commands::routing::routing_followups,
            commands::routing::routing_followup_draft,
            commands::routing::routing_followup_done,
            commands::routing::gig_recap_get,
            commands::routing::gig_recap_save,
            commands::routing::played_at_list,
            commands::routing::played_at_push,
            // offers, documents, payments, advancing (commands/offers)
            commands::offers::list_offers,
            commands::offers::get_offer,
            commands::offers::new_offer_for_contact,
            commands::offers::save_offer,
            commands::offers::set_offer_status,
            commands::offers::delete_offer,
            commands::offers::money::list_offer_payments,
            commands::offers::money::set_offer_payment,
            commands::offers::money::list_offer_invoices,
            commands::offers::advancing::list_advancing,
            commands::offers::advancing::get_event_advancing,
            commands::offers::advancing::set_advancing_item,
            commands::offers::docs::generate_offer_document,
            commands::offers::docs::list_offer_documents,
            commands::offers::docs::open_offer_document,
            commands::offers::docs::offer_email_draft,
            commands::offers::docs::advancing_email_draft,
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
            commands::vi::vi_exclude_venue,
            commands::vi::vi_restore_venue,
            commands::vi::vi_exclude_email,
            // venue intelligence (J2, harvest)
            commands::harvest::vi_resolve_area,
            commands::harvest::vi_resolve_all_areas,
            commands::harvest::vi_start_harvest,
            commands::harvest::vi_resume_run,
            commands::harvest::vi_list_runs,
            commands::harvest::vi_list_venues,
            commands::harvest::vi_count_venues,
            commands::harvest::vi_repair_contacts,
            commands::harvest::vi_venue_stats,
            commands::harvest::vi_promote_venue,
            commands::harvest::vi_promote_venues_bulk,
            commands::harvest::vi_stop_run,
            commands::harvest::vi_run_tasks,
            // pulse (spotify snapshots + meta spend)
            commands::pulse::pulse_snapshot,
            commands::pulse::pulse_kpis,
            commands::pulse::pulse_series,
            commands::pulse::pulse_tracked_list,
            commands::pulse::pulse_tracked_add,
            commands::pulse::pulse_tracked_toggle,
            commands::pulse::pulse_watchlist,
            commands::pulse::pulse_watchlist_add,
            commands::pulse::pulse_watchlist_toggle,
            // radar (curators/labels)
            commands::radar::radar_harvest,
            commands::radar::radar_enrich,
            commands::radar::radar_list,
            commands::radar::radar_stats,
            commands::radar::radar_promote,
            commands::radar::radar_promote_bulk,
            commands::radar::radar_repair_contacts,
            commands::radar::radar_reject,
            commands::radar::radar_restore,
            commands::radar::radar_unpromote,
            // venue intelligence (J5, enrichment)
            commands::enrich::vi_start_enrich,
            commands::enrich::vi_venue_detail,
            // promoters / collectives, fit score, comparable artists
            commands::promoters::vi_list_promoters,
            commands::promoters::vi_count_promoters,
            commands::promoters::vi_promoter_detail,
            commands::promoters::vi_promote_promoter,
            commands::promoters::vi_start_promoter_enrich,
            commands::fit::fit_recompute,
            commands::fit::fit_list_comparables,
            commands::fit::fit_add_comparable,
            commands::fit::fit_delete_comparable,
            commands::fit::fit_suggest_comparables,
            commands::fit::fit_seed_comparables,
            // templates
            commands::data::list_templates,
            commands::data::save_template,
            commands::data::delete_template,
            // budget
            commands::data::list_budget,
            commands::data::save_budget_item,
            commands::data::delete_budget_item,
            commands::data::link_budget_item,
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
            menu::set_menu_lang,
            commands::data::dashboard_stats,
            // iPhone notifications (Pushover, commands/push.rs)
            commands::push::push_get,
            commands::push::push_save,
            commands::push::push_test,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PiersCRM");
}
