mod acp;
mod agents;
mod chatstore;
mod commands;
mod engine;
mod files;
mod projectstore;
mod runstore;
mod state;

use tauri::menu::{Menu, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, RunEvent, WebviewUrl, WebviewWindowBuilder, Wry};
use uuid::Uuid;

/// GUI apps launched from Finder inherit a minimal PATH that omits where
/// claude/codex/gemini are installed (~/.local/bin, nvm, etc.). Pull the login
/// shell's PATH so the CLIs resolve at runtime.
#[cfg(target_os = "macos")]
fn fix_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    if let Ok(output) = std::process::Command::new(&shell)
        .args(["-lic", "echo $PATH"])
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                std::env::set_var("PATH", path);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn fix_path() {}

/// Open a fresh Arthur window. Label is UUID-based so spawning never collides
/// with a still-live window (Tauri rejects re-using a live label).
fn spawn_window(app: &AppHandle) -> Result<String, String> {
    let label = format!("arthur-{}", Uuid::new_v4().simple());
    WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("arthur")
        .inner_size(1100.0, 720.0)
        .focused(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(label)
}

/// Native menu: App (macOS), File (New Window / Close), Edit (text-input
/// shortcuts), Window. The Edit submenu is required on macOS — without it,
/// Cmd+C/V/X don't work in text inputs.
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let new_window = MenuItem::with_id(
        app,
        "new_window",
        "New Window",
        true,
        Some("CmdOrCtrl+N"),
    )?;

    let mut menu = MenuBuilder::new(app);

    #[cfg(target_os = "macos")]
    {
        let app_submenu = SubmenuBuilder::new(app, "arthur")
            .item(&PredefinedMenuItem::about(app, None, None)?)
            .separator()
            .item(&PredefinedMenuItem::hide(app, None)?)
            .item(&PredefinedMenuItem::hide_others(app, None)?)
            .item(&PredefinedMenuItem::show_all(app, None)?)
            .separator()
            .item(&PredefinedMenuItem::quit(app, None)?)
            .build()?;
        menu = menu.item(&app_submenu);
    }

    let file_submenu = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .separator()
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .build()?;

    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;

    let window_submenu = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, Some("Zoom"))?)
        .build()?;

    menu.item(&file_submenu)
        .item(&edit_submenu)
        .item(&window_submenu)
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    fix_path();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        .menu(build_menu)
        .on_menu_event(|app, event| {
            if event.id().0.as_str() == "new_window" {
                if let Err(e) = spawn_window(app) {
                    eprintln!("failed to spawn window: {e}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_agents,
            commands::list_workflows,
            commands::get_workflow,
            commands::read_workflow_source,
            commands::parse_workflow_source,
            commands::save_workflow,
            commands::create_workflow,
            commands::improve_workflow,
            commands::cancel_improve,
            commands::generate_chat_title,
            commands::start_chat,
            commands::cancel_chat,
            commands::close_chat,
            commands::respond_permission,
            commands::list_chats,
            commands::load_chat,
            commands::save_chat,
            commands::delete_chat,
            commands::list_project_files,
            commands::list_slash_commands,
            commands::start_run,
            commands::approve,
            commands::respond_ask,
            commands::respond_message,
            commands::cancel,
            commands::list_changed_files,
            commands::list_all_files,
            commands::read_file_preview,
            commands::diff_file,
            commands::reset_files_baseline,
            commands::git_current_branch,
            commands::list_recent_projects,
            commands::add_recent_project,
            commands::remove_recent_project,
            commands::clear_recent_projects,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // On macOS, clicking the dock icon with no windows open fires Reopen.
    // Spawn a fresh window so the app remains usable after closing the last one.
    app.run(|app, event| {
        if let RunEvent::Reopen {
            has_visible_windows,
            ..
        } = event
        {
            if !has_visible_windows {
                if let Err(e) = spawn_window(app) {
                    eprintln!("failed to spawn window on reopen: {e}");
                }
            }
        }
    });
}
