//! The menu bar extra: Prompting Time's icon on the right of the macOS menu bar. Its menu lists each
//! background agent with its status, its Pause or Resume (Start while it is stopped) and Poll now,
//! how many reviews need you, Open Prompting Time, and Quit. The window sends what its sidebar shows
//! (`UpdateMenuBarRequest`), so the menu reads the same plugin status in the same words. A choice
//! that acts on an agent or opens a screen goes back to the window as a `MenuBarCommand`, which the
//! window runs as the sidebar would.
//!
//! The icon is a template image: macOS draws it in the menu bar's own color, so it suits light and
//! dark menu bars, and a dot marks it while an agent fails, is blocked, or can't start.

use std::sync::{Mutex, PoisonError};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{MenuBarCommand, UpdateMenuBarRequest};

/// The event that carries a `MenuBarCommand` to the window.
pub const MENU_BAR_EVENT: &str = "prompting-time://menu-bar";
const TRAY_ID: &str = "background-agents";
const OPEN_ID: &str = "open";
const QUIT_ID: &str = "quit";
/// Black on clear at 36 pixels, which macOS draws 18 points tall: a clock, as in the app's icon.
const ICON: Image<'static> = tauri::include_image!("icons/menu-bar.png");
/// The clock with a dot at its top right.
const ALERT_ICON: Image<'static> = tauri::include_image!("icons/menu-bar-alert.png");

/// The icon while the window says to show it, and what it shows.
#[derive(Default)]
pub struct MenuBar {
    shown: Mutex<Option<Shown>>,
}

struct Shown {
    tray: TrayIcon,
    request: UpdateMenuBarRequest,
}

impl MenuBar {
    /// Shows `request` in the menu bar, or removes the icon when it says not to show it. An
    /// unchanged request changes nothing, so a menu the user has open stays as it is. Tauri runs this
    /// synchronous command on the main thread, where AppKit wants the icon added and dropped.
    pub fn update(&self, app: &AppHandle, request: UpdateMenuBarRequest) -> tauri::Result<()> {
        let mut shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        if !request.visible {
            if let Some(removed) = shown.take() {
                // Tauri keeps a copy of the icon too; the icon leaves the menu bar once both drop.
                drop(app.remove_tray_by_id(removed.tray.id()));
            }
            return Ok(());
        }
        if shown
            .as_ref()
            .is_some_and(|current| current.request == request)
        {
            return Ok(());
        }
        let menu = build_menu(app, &lines(&request))?;
        match shown.as_mut() {
            Some(current) => {
                current.tray.set_menu(Some(menu))?;
                if alert(&current.request) != alert(&request) {
                    // set_icon alone would draw the new image as a plain one, not a template.
                    current
                        .tray
                        .set_icon_with_as_template(Some(icon(&request)), true)?;
                }
                current.tray.set_tooltip(Some(tooltip(&request)))?;
                current.request = request;
            }
            None => {
                let tray = TrayIconBuilder::with_id(TRAY_ID)
                    .icon(icon(&request))
                    .icon_as_template(true)
                    .tooltip(tooltip(&request))
                    .menu(&menu)
                    .build(app)?;
                *shown = Some(Shown { tray, request });
            }
        }
        Ok(())
    }
}

/// Carries out a click on the menu item `id`. Every menu's clicks arrive here, so an ID that is not
/// the menu bar extra's does nothing.
pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match choice(id) {
        Some(Choice::Open) => show_window(app),
        Some(Choice::Quit) => app.exit(0),
        Some(Choice::Window(command)) => {
            // Pause, Resume, Start, and Poll now run without taking the user away from what they do.
            if !matches!(command, MenuBarCommand::Agent { .. }) {
                show_window(app);
            }
            let _ = app.emit(MENU_BAR_EVENT, command);
        }
        None => {}
    }
}

/// One line of the menu.
#[derive(Debug, Eq, PartialEq)]
enum Line {
    Item {
        id: String,
        text: String,
        enabled: bool,
    },
    Separator,
}

/// What a click on a menu item does.
#[derive(Debug, Eq, PartialEq)]
enum Choice {
    Open,
    Quit,
    Window(MenuBarCommand),
}

/// The menu for `request`. For each agent: its title, which opens its screen; its status words,
/// which open the screen of a cause that blocks every review and are inert otherwise; and its row's
/// buttons. Then how many reviews need you, which opens the first; then Open Prompting Time and Quit.
fn lines(request: &UpdateMenuBarRequest) -> Vec<Line> {
    let item = |id: String, text: &str, enabled: bool| Line::Item {
        id,
        text: menu_text(text),
        enabled,
    };
    let mut lines = Vec::new();
    for agent in &request.agents {
        lines.push(item(
            command_id(&MenuBarCommand::OpenAgent {
                plugin: agent.plugin.clone(),
            }),
            &agent.title,
            true,
        ));
        lines.push(match &agent.cause {
            Some(cause) => item(
                command_id(&MenuBarCommand::OpenCause {
                    plugin: agent.plugin.clone(),
                    cause: cause.clone(),
                }),
                &agent.status,
                true,
            ),
            None => item(format!("status:{}", agent.plugin), &agent.status, false),
        });
        for control in &agent.controls {
            lines.push(item(
                command_id(&MenuBarCommand::Agent {
                    plugin: agent.plugin.clone(),
                    action: control.action,
                }),
                &control.label,
                control.enabled,
            ));
        }
        lines.push(Line::Separator);
    }
    let reviews = match request.reviews_needing_you {
        0 => "No reviews need you".to_owned(),
        1 => "1 review needs you".to_owned(),
        count => format!("{count} reviews need you"),
    };
    lines.push(item(
        command_id(&MenuBarCommand::OpenNeedsYou),
        &reviews,
        request.reviews_needing_you > 0,
    ));
    lines.push(Line::Separator);
    lines.push(item(OPEN_ID.to_owned(), "Open Prompting Time", true));
    lines.push(item(QUIT_ID.to_owned(), "Quit Prompting Time", true));
    lines
}

/// A command's menu item ID is the command as JSON, so a click reads it back whole.
fn command_id(command: &MenuBarCommand) -> String {
    serde_json::to_string(command).expect("a menu bar command serializes")
}

fn choice(id: &str) -> Option<Choice> {
    match id {
        OPEN_ID => Some(Choice::Open),
        QUIT_ID => Some(Choice::Quit),
        _ => serde_json::from_str(id).ok().map(Choice::Window),
    }
}

/// AppKit menus drop a single "&", which marks a Windows shortcut letter, and show "&&" as "&".
fn menu_text(text: &str) -> String {
    text.replace('&', "&&")
}

fn alert(request: &UpdateMenuBarRequest) -> bool {
    request.agents.iter().any(|agent| agent.alert)
}

fn icon(request: &UpdateMenuBarRequest) -> Image<'static> {
    if alert(request) { ALERT_ICON } else { ICON }
}

/// "Prompting Time", then each agent and its status words on a line of its own.
fn tooltip(request: &UpdateMenuBarRequest) -> String {
    std::iter::once("Prompting Time".to_owned())
        .chain(
            request
                .agents
                .iter()
                .map(|agent| format!("{}: {}", agent.title, agent.status)),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_menu(app: &AppHandle, lines: &[Line]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for line in lines {
        match line {
            Line::Item { id, text, enabled } => {
                menu.append(&MenuItem::with_id(
                    app,
                    id.as_str(),
                    text,
                    *enabled,
                    None::<&str>,
                )?)?;
            }
            Line::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(menu)
}

/// Brings the window forward, also when the app is hidden or the window minimized.
fn show_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.show();
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{MenuBarAction, MenuBarAgent, MenuBarControl};

    fn control(action: MenuBarAction, label: &str, enabled: bool) -> MenuBarControl {
        MenuBarControl {
            action,
            label: label.to_owned(),
            enabled,
        }
    }

    fn agent(status: &str) -> MenuBarAgent {
        MenuBarAgent {
            plugin: "whooper".to_owned(),
            title: "Whooper".to_owned(),
            status: status.to_owned(),
            cause: None,
            alert: false,
            controls: vec![
                control(MenuBarAction::Pause, "Pause", true),
                control(MenuBarAction::Poll, "Poll now", true),
            ],
        }
    }

    fn request(agents: Vec<MenuBarAgent>, reviews_needing_you: u32) -> UpdateMenuBarRequest {
        UpdateMenuBarRequest {
            visible: true,
            agents,
            reviews_needing_you,
        }
    }

    /// Each line as the menu shows it: "-" for a separator, and an item's text with "(off)" when
    /// it is disabled.
    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match line {
                Line::Separator => "-".to_owned(),
                Line::Item { text, enabled, .. } => {
                    format!("{text}{}", if *enabled { "" } else { " (off)" })
                }
            })
            .collect()
    }

    fn choice_of(lines: &[Line], text: &str) -> Option<Choice> {
        lines.iter().find_map(|line| match line {
            Line::Item {
                id, text: shown, ..
            } if shown == text => choice(id),
            _ => None,
        })
    }

    #[test]
    fn the_menu_lists_each_agent_with_its_status_and_buttons_then_the_reviews_open_and_quit() {
        let lines = lines(&request(vec![agent("Running · next poll 3:45 PM")], 3));
        assert_eq!(
            texts(&lines),
            [
                "Whooper",
                "Running · next poll 3:45 PM (off)",
                "Pause",
                "Poll now",
                "-",
                "3 reviews need you",
                "-",
                "Open Prompting Time",
                "Quit Prompting Time",
            ]
        );
        assert_eq!(
            choice_of(&lines, "Pause"),
            Some(Choice::Window(MenuBarCommand::Agent {
                plugin: "whooper".to_owned(),
                action: MenuBarAction::Pause,
            }))
        );
        assert_eq!(
            choice_of(&lines, "Poll now"),
            Some(Choice::Window(MenuBarCommand::Agent {
                plugin: "whooper".to_owned(),
                action: MenuBarAction::Poll,
            }))
        );
        assert_eq!(
            choice_of(&lines, "Whooper"),
            Some(Choice::Window(MenuBarCommand::OpenAgent {
                plugin: "whooper".to_owned()
            }))
        );
        assert_eq!(
            choice_of(&lines, "3 reviews need you"),
            Some(Choice::Window(MenuBarCommand::OpenNeedsYou))
        );
        assert_eq!(choice_of(&lines, "Open Prompting Time"), Some(Choice::Open));
        assert_eq!(choice_of(&lines, "Quit Prompting Time"), Some(Choice::Quit));
    }

    #[test]
    fn the_buttons_follow_the_row_and_the_review_count_reads_as_a_sentence() {
        let paused = MenuBarAgent {
            controls: vec![
                control(MenuBarAction::Resume, "Resume", true),
                control(MenuBarAction::Poll, "Poll now", true),
            ],
            ..agent("Paused")
        };
        let polling = MenuBarAgent {
            controls: vec![
                control(MenuBarAction::Pause, "Pause", false),
                control(MenuBarAction::Poll, "Polling…", false),
            ],
            ..agent("Polling now")
        };
        let stopped = MenuBarAgent {
            controls: vec![
                control(MenuBarAction::Start, "Start", true),
                control(MenuBarAction::Poll, "Poll now", false),
            ],
            ..agent("Stopped")
        };
        let without_actions = MenuBarAgent {
            controls: Vec::new(),
            ..agent("Running")
        };
        assert_eq!(
            texts(&lines(&request(vec![paused], 1)))[..6],
            [
                "Whooper",
                "Paused (off)",
                "Resume",
                "Poll now",
                "-",
                "1 review needs you"
            ]
        );
        assert_eq!(
            texts(&lines(&request(vec![polling], 0)))[..6],
            [
                "Whooper",
                "Polling now (off)",
                "Pause (off)",
                "Polling… (off)",
                "-",
                "No reviews need you (off)"
            ]
        );
        assert_eq!(
            texts(&lines(&request(vec![stopped], 0)))[2..4],
            ["Start", "Poll now (off)"]
        );
        assert_eq!(
            texts(&lines(&request(vec![without_actions], 0)))[..3],
            ["Whooper", "Running (off)", "-"]
        );
    }

    #[test]
    fn a_blocking_cause_opens_its_screen_and_marks_the_icon() {
        let blocked = MenuBarAgent {
            status: "Can't review · Docker & gh aren't ready".to_owned(),
            cause: Some("docker:setup".to_owned()),
            alert: true,
            ..agent("")
        };
        let request = request(vec![blocked.clone()], 0);
        let lines = lines(&request);
        // AppKit shows "&&" as one "&".
        let status = "Can't review · Docker && gh aren't ready";
        assert_eq!(texts(&lines)[1], status);
        assert_eq!(
            choice_of(&lines, status),
            Some(Choice::Window(MenuBarCommand::OpenCause {
                plugin: "whooper".to_owned(),
                cause: "docker:setup".to_owned(),
            }))
        );
        assert!(alert(&request));
        // One agent in trouble is enough for the dot.
        assert!(alert(&self::request(vec![agent("Running"), blocked], 0)));
        assert!(!alert(&self::request(vec![agent("Running")], 0)));
        assert_eq!(
            tooltip(&request),
            "Prompting Time\nWhooper: Can't review · Docker & gh aren't ready"
        );
    }

    #[test]
    fn each_agent_has_its_own_block_and_every_enabled_item_does_something() {
        let other = MenuBarAgent {
            plugin: "second-agent".to_owned(),
            title: "Second agent".to_owned(),
            ..agent("Stopped")
        };
        let lines = lines(&request(vec![agent("Running"), other], 2));
        assert_eq!(
            texts(&lines)[..10],
            [
                "Whooper",
                "Running (off)",
                "Pause",
                "Poll now",
                "-",
                "Second agent",
                "Stopped (off)",
                "Pause",
                "Poll now",
                "-",
            ]
        );
        let pauses: Vec<_> = lines
            .iter()
            .filter_map(|line| match line {
                Line::Item { id, text, .. } if text == "Pause" => choice(id),
                _ => None,
            })
            .collect();
        assert_eq!(
            pauses,
            [
                Choice::Window(MenuBarCommand::Agent {
                    plugin: "whooper".to_owned(),
                    action: MenuBarAction::Pause
                }),
                Choice::Window(MenuBarCommand::Agent {
                    plugin: "second-agent".to_owned(),
                    action: MenuBarAction::Pause
                }),
            ]
        );
        for line in &lines {
            if let Line::Item {
                id,
                text,
                enabled: true,
            } = line
            {
                assert!(choice(id).is_some(), "{text} does nothing");
            }
        }
    }

    #[test]
    fn other_menus_ids_do_nothing() {
        for id in [
            "",
            "status:whooper",
            "close",
            "{\"kind\":\"agent\"}",
            "null",
        ] {
            assert_eq!(choice(id), None, "{id}");
        }
    }

    #[test]
    fn commands_reach_the_window_in_its_format() {
        assert_eq!(
            serde_json::to_value(MenuBarCommand::Agent {
                plugin: "whooper".to_owned(),
                action: MenuBarAction::Poll,
            })
            .unwrap(),
            serde_json::json!({ "kind": "agent", "plugin": "whooper", "action": "poll" })
        );
        assert_eq!(
            serde_json::to_value(MenuBarCommand::OpenCause {
                plugin: "whooper".to_owned(),
                cause: "docker".to_owned(),
            })
            .unwrap(),
            serde_json::json!({ "kind": "openCause", "plugin": "whooper", "cause": "docker" })
        );
        assert_eq!(
            serde_json::to_value(MenuBarCommand::OpenNeedsYou).unwrap(),
            serde_json::json!({ "kind": "openNeedsYou" })
        );
        let request: UpdateMenuBarRequest = serde_json::from_value(serde_json::json!({
            "visible": true,
            "reviewsNeedingYou": 2,
            "agents": [{
                "plugin": "whooper", "title": "Whooper", "status": "Paused", "cause": null, "alert": false,
                "controls": [{ "action": "resume", "label": "Resume", "enabled": true }],
            }],
        }))
        .unwrap();
        assert_eq!(request.reviews_needing_you, 2);
        assert_eq!(request.agents[0].controls[0].action, MenuBarAction::Resume);
    }

    #[test]
    fn the_icons_are_templates_and_only_the_alert_has_the_dot() {
        for image in [&ICON, &ALERT_ICON] {
            assert_eq!((image.width(), image.height()), (36, 36));
            // A template's color is ignored; macOS draws its alpha in the menu bar's color.
            assert!(image.rgba().chunks(4).all(|pixel| pixel[..3] == [0, 0, 0]));
        }
        let alpha = |image: &Image<'_>, x: usize, y: usize| image.rgba()[(y * 36 + x) * 4 + 3];
        // The dot's center, top right.
        assert_eq!(alpha(&ALERT_ICON, 30, 6), 255);
        assert_eq!(alpha(&ICON, 30, 6), 0);
    }
}
