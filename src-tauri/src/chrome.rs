//! Window chrome (macOS): keeps the traffic lights inside the floating glass
//! sidebar.
//!
//! `trafficLightPosition` in tauri.conf.json is applied by tao only on its own
//! view's `drawRect:`. With a transparent window fully covered by the webview
//! that redraw rarely happens, so after AppKit re-lays the title bar (theme
//! change, focus, resize) the buttons fall back to their default spot, half
//! outside the sidebar. We re-apply the same inset ourselves on those events.

/// Keep in sync with `trafficLightPosition` and the sidebar inset (Layout.tsx).
#[cfg(target_os = "macos")]
const LIGHTS_X: f64 = 20.0;
#[cfg(target_os = "macos")]
const LIGHTS_Y: f64 = 20.0;

#[cfg(target_os = "macos")]
fn inset_now(window: &tauri::Window) {
    use objc2_app_kit::{NSWindow, NSWindowButton};

    let Ok(ptr) = window.ns_window() else { return };
    if ptr.is_null() {
        return;
    }
    // SAFETY: called on the main thread with the live NSWindow of this window.
    unsafe {
        let ns: &NSWindow = &*(ptr as *const NSWindow);
        let (Some(close), Some(mini), Some(zoom)) = (
            ns.standardWindowButton(NSWindowButton::CloseButton),
            ns.standardWindowButton(NSWindowButton::MiniaturizeButton),
            ns.standardWindowButton(NSWindowButton::ZoomButton),
        ) else {
            return;
        };
        let Some(container) = close.superview().and_then(|v| v.superview()) else {
            return;
        };
        let close_rect = close.frame();
        let height = close_rect.size.height + LIGHTS_Y;
        let mut bar = container.frame();
        bar.size.height = height;
        bar.origin.y = ns.frame().size.height - height;
        container.setFrame(bar);

        let gap = mini.frame().origin.x - close_rect.origin.x;
        for (i, button) in [close, mini, zoom].iter().enumerate() {
            let mut origin = button.frame().origin;
            origin.x = LIGHTS_X + i as f64 * gap;
            button.setFrameOrigin(origin);
        }
    }
}

/// Re-applies the inset now and once more shortly after: AppKit finishes some
/// title bar layouts (appearance change) after the event that triggered them.
pub fn place_traffic_lights(window: &tauri::Window) {
    #[cfg(target_os = "macos")]
    {
        let w = window.clone();
        let _ = window.run_on_main_thread(move || inset_now(&w));
        let w = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            let w2 = w.clone();
            let _ = w.run_on_main_thread(move || inset_now(&w2));
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Called by the frontend right after it changes the window theme.
#[tauri::command]
pub fn place_window_controls(window: tauri::Window) {
    place_traffic_lights(&window);
}
