mod acp;
mod agents;
mod chatstore;
mod commands;
mod engine;
mod files;
mod runstore;
mod state;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    fix_path();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
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
            commands::cancel,
            commands::list_changed_files,
            commands::list_all_files,
            commands::read_file_preview,
            commands::diff_file,
            commands::reset_files_baseline,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
