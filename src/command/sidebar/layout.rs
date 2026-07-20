//! Sidebar-preserving tmux layout application.

use anyhow::{Context, Result, anyhow, bail};

use crate::cmd::Cmd;
use crate::config::SidebarPosition;

use super::panes::find_sidebar_pane_id;

fn canonical_window_id(target: Option<&str>) -> Result<String> {
    let mut cmd = Cmd::new("tmux").arg("display-message");
    if let Some(target) = target {
        cmd = cmd.args(&["-t", target]);
    }
    let window_id = cmd.args(&["-p", "#{window_id}"]).run_and_capture_stdout()?;
    if window_id.is_empty() {
        bail!("could not detect tmux window");
    }
    Ok(window_id)
}

fn content_pane_id(window_id: &str, sidebar_pane_id: &str) -> Result<String> {
    let panes = Cmd::new("tmux")
        .args(&["list-panes", "-t", window_id, "-F", "#{pane_id}"])
        .run_and_capture_stdout()?;

    panes
        .lines()
        .find(|pane_id| *pane_id != sidebar_pane_id)
        .map(String::from)
        .ok_or_else(|| anyhow!("cannot change layout in a window with only a sidebar pane"))
}

fn select_layout(window_id: &str, layout: &str) -> Result<()> {
    let result = match layout {
        "next" => Cmd::new("tmux")
            .args(&["select-layout", "-n", "-t", window_id])
            .run(),
        "previous" => Cmd::new("tmux")
            .args(&["select-layout", "-p", "-t", window_id])
            .run(),
        layout => Cmd::new("tmux")
            .args(&["select-layout", "-t", window_id, layout])
            .run(),
    };
    result.map(|_| ())
}

fn join_sidebar(
    window_id: &str,
    content_pane_id: &str,
    sidebar_pane_id: &str,
    position: SidebarPosition,
    size: u16,
) -> Result<()> {
    let split_flag = match position {
        SidebarPosition::Left => "-hbf",
        SidebarPosition::Top => "-vbf",
    };
    let size = size.to_string();

    Cmd::new("tmux")
        .args(&[
            "join-pane",
            split_flag,
            "-d",
            "-l",
            &size,
            "-s",
            sidebar_pane_id,
            "-t",
            content_pane_id,
        ])
        .run()
        .with_context(|| format!("failed to restore sidebar to window {window_id}"))?;
    Ok(())
}

pub(super) fn apply(layout: &str, target: Option<&str>) -> Result<()> {
    if std::env::var("TMUX").is_err() {
        bail!("Sidebar requires tmux");
    }

    let window_id = canonical_window_id(target)?;
    let Some(sidebar_pane_id) = find_sidebar_pane_id(&window_id)? else {
        return select_layout(&window_id, layout);
    };

    let content_pane_id = content_pane_id(&window_id, &sidebar_pane_id)?;
    let (position, size) = super::get_sidebar_position_and_size(&window_id)?;
    let temporary_window = Cmd::new("tmux")
        .args(&[
            "break-pane",
            "-d",
            "-P",
            "-F",
            "#{window_id}",
            "-s",
            &sidebar_pane_id,
        ])
        .run_and_capture_stdout()?;

    let layout_result = select_layout(&window_id, layout);
    let restore_result = join_sidebar(
        &window_id,
        &content_pane_id,
        &sidebar_pane_id,
        position,
        size,
    );

    if let Err(restore_error) = restore_result {
        let layout_error = layout_result
            .as_ref()
            .err()
            .map(|error| format!("; layout action also failed: {error}"))
            .unwrap_or_default();
        return Err(restore_error).context(format!(
            "sidebar pane {sidebar_pane_id} remains in temporary window {temporary_window}{layout_error}"
        ));
    }

    super::layout_tree::reflow_after_sidebar_add(&window_id, &sidebar_pane_id, position, size);
    layout_result
}
