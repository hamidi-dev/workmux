//! Formatting helpers for dashboard UI rendering.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row};

/// Truncate a string to max_len characters, appending ellipsis if truncated.
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() > max_len {
        s.chars().take(max_len - 1).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

use crate::config::StatusIcons;
use crate::git::GitStatus;
use crate::github::{CheckSummary, PrSummary};
use crate::multiplexer::AgentStatus;
use crate::nerdfont;
use crate::nerdfont::GitIcons;
use crate::ui::pr_status::{PrStatusOptions, format_pr_details as shared_format_pr_details};
use crate::workflow::types::AgentStatusSummary;

use super::super::ansi;
use super::super::spinner::SPINNER_FRAMES;
use super::theme::ThemePalette;

/// Spacing mode for agent status icon rendering.
pub enum AgentStatusFormat {
    /// Table cell: trailing space after each icon, spinner padded with spaces.
    TableCell,
    /// Detail line: single space separators between present statuses only.
    DetailLine,
}

struct AgentStatusCounts {
    working: usize,
    waiting: usize,
    done: usize,
}

fn count_agent_statuses(summary: &AgentStatusSummary) -> AgentStatusCounts {
    AgentStatusCounts {
        working: summary
            .statuses
            .iter()
            .filter(|s| **s == AgentStatus::Working)
            .count(),
        waiting: summary
            .statuses
            .iter()
            .filter(|s| **s == AgentStatus::Waiting)
            .count(),
        done: summary
            .statuses
            .iter()
            .filter(|s| **s == AgentStatus::Done)
            .count(),
    }
}

/// Format agent status icons for dashboard table cells and detail lines.
pub fn format_agent_status_summary(
    summary: Option<&AgentStatusSummary>,
    icons: &StatusIcons,
    spinner_frame: u8,
    palette: &ThemePalette,
    mode: AgentStatusFormat,
) -> Vec<(String, Style)> {
    let Some(summary) = summary else {
        return if matches!(mode, AgentStatusFormat::TableCell) {
            vec![("-".to_string(), Style::default().fg(palette.dimmed))]
        } else {
            Vec::new()
        };
    };

    let counts = count_agent_statuses(summary);
    let spinner = SPINNER_FRAMES[spinner_frame as usize % SPINNER_FRAMES.len()];
    let separator_style = Style::default().fg(palette.text);
    let mut parts: Vec<(String, Style)> = Vec::new();
    let mut has_prior = false;

    if counts.working > 0 {
        let icon = icons.working();
        let base_style = Style::default().fg(palette.info);
        parts.extend(ansi::parse_tmux_styles(icon, base_style));
        match mode {
            AgentStatusFormat::TableCell => {
                parts.push((format!(" {} ", spinner), base_style));
            }
            AgentStatusFormat::DetailLine => {
                parts.push((format!(" {}", spinner), base_style));
            }
        }
        has_prior = true;
    }
    if counts.waiting > 0 {
        if matches!(mode, AgentStatusFormat::DetailLine) && has_prior {
            parts.push((" ".to_string(), separator_style));
        }
        let icon = icons.waiting();
        let base_style = Style::default().fg(palette.accent);
        parts.extend(ansi::parse_tmux_styles(icon, base_style));
        if matches!(mode, AgentStatusFormat::TableCell) {
            parts.push((" ".to_string(), base_style));
        }
        has_prior = true;
    }
    if counts.done > 0 {
        if matches!(mode, AgentStatusFormat::DetailLine) && has_prior {
            parts.push((" ".to_string(), separator_style));
        }
        let icon = icons.done();
        let base_style = Style::default().fg(palette.success);
        parts.extend(ansi::parse_tmux_styles(icon, base_style));
        if matches!(mode, AgentStatusFormat::TableCell) {
            parts.push((" ".to_string(), base_style));
        }
    }

    if parts.is_empty() && matches!(mode, AgentStatusFormat::TableCell) {
        parts.push(("-".to_string(), Style::default().fg(palette.dimmed)));
    }

    parts
}

/// Add diff icon and uncommitted (+N/-N) spans. Caller is responsible for
/// the leading separator if needed.
fn add_uncommitted_spans(
    spans: &mut Vec<(String, Style)>,
    status: &GitStatus,
    icons: &GitIcons,
    palette: &ThemePalette,
) {
    spans.push((icons.diff.to_string(), Style::default().fg(palette.accent)));

    if status.uncommitted_added > 0 {
        spans.push((" ".to_string(), Style::default()));
        spans.push((
            format!("+{}", status.uncommitted_added),
            Style::default().fg(palette.success),
        ));
    }
    if status.uncommitted_removed > 0 {
        spans.push((" ".to_string(), Style::default()));
        spans.push((
            format!("-{}", status.uncommitted_removed),
            Style::default().fg(palette.danger),
        ));
    }
}

/// Format git status for the Git column: base branch, diff stats, then indicators
/// Format: "→branch +N -M 󰏫 +X -Y 󰀪 ↑A ↓B"
/// When there are uncommitted changes that differ from total, branch totals are dimmed
pub fn format_git_status(
    status: Option<&GitStatus>,
    spinner_frame: u8,
    palette: &ThemePalette,
) -> Vec<(String, Style)> {
    let icons = nerdfont::git_icons();

    if let Some(status) = status {
        let mut spans: Vec<(String, Style)> = Vec::new();
        let has_uncommitted =
            status.uncommitted_added > 0 || status.uncommitted_removed > 0 || status.is_dirty;

        // Check if uncommitted equals total (all changes are uncommitted, nothing committed yet)
        let all_uncommitted = status.uncommitted_added == status.lines_added
            && status.uncommitted_removed == status.lines_removed;

        // Rebase indicator (shown first, before everything else)
        if status.is_rebasing {
            spans.push((
                icons.rebase.to_string(),
                Style::default().fg(palette.warning),
            ));
        }

        // Base branch (dimmed) - only show if not default (main/master)
        if !status.base_branch.is_empty()
            && status.base_branch != "main"
            && status.base_branch != "master"
        {
            spans.push((
                format!("→{}", status.base_branch),
                Style::default().fg(palette.dimmed),
            ));
        }

        // Always dim branch totals (historical), always bright uncommitted (active work)
        // - Clean: dim branch totals only
        // - All uncommitted: icon + bright uncommitted only
        // - Mixed: dim branch totals + icon + bright uncommitted
        if has_uncommitted && all_uncommitted {
            // All changes are uncommitted - show icon + bright numbers only
            if !spans.is_empty() {
                spans.push((" ".to_string(), Style::default()));
            }
            add_uncommitted_spans(&mut spans, status, &icons, palette);
        } else {
            // Either clean or mixed - show dim branch totals
            if status.lines_added > 0 {
                if !spans.is_empty() {
                    spans.push((" ".to_string(), Style::default()));
                }
                spans.push((
                    format!("+{}", status.lines_added),
                    Style::default()
                        .fg(palette.success)
                        .add_modifier(Modifier::DIM),
                ));
            }
            if status.lines_removed > 0 {
                if !spans.is_empty() {
                    spans.push((" ".to_string(), Style::default()));
                }
                spans.push((
                    format!("-{}", status.lines_removed),
                    Style::default()
                        .fg(palette.danger)
                        .add_modifier(Modifier::DIM),
                ));
            }

            // If there are uncommitted changes, show icon + bright uncommitted
            if has_uncommitted {
                if !spans.is_empty() {
                    spans.push((" ".to_string(), Style::default()));
                }
                add_uncommitted_spans(&mut spans, status, &icons, palette);
            }
        }

        // Conflict indicator
        if status.has_conflict {
            if !spans.is_empty() {
                spans.push((" ".to_string(), Style::default()));
            }
            spans.push((
                icons.conflict.to_string(),
                Style::default().fg(palette.danger),
            ));
        }

        // Ahead/behind upstream
        if status.ahead > 0 {
            if !spans.is_empty() {
                spans.push((" ".to_string(), Style::default()));
            }
            spans.push((
                format!("↑{}", status.ahead),
                Style::default().fg(palette.info),
            ));
        }
        if status.behind > 0 {
            if !spans.is_empty() {
                spans.push((" ".to_string(), Style::default()));
            }
            spans.push((
                format!("↓{}", status.behind),
                Style::default().fg(palette.warning),
            ));
        }

        if spans.is_empty() {
            vec![("-".to_string(), Style::default().fg(palette.dimmed))]
        } else {
            spans
        }
    } else {
        // No status yet - show spinner
        let frame = SPINNER_FRAMES[spinner_frame as usize % SPINNER_FRAMES.len()];
        vec![(frame.to_string(), Style::default().fg(palette.dimmed))]
    }
}

/// Format GitHub PR and check status as styled spans for dashboard display
pub fn format_pr_status(
    pr: Option<&PrSummary>,
    checks: Option<&CheckSummary>,
    show_check_counts: bool,
    spinner_frame: u8,
    palette: &ThemePalette,
) -> Vec<(String, Style)> {
    crate::ui::pr_status::format_github_status(
        pr,
        checks,
        PrStatusOptions {
            include_number: true,
            show_check_counts,
            none_placeholder: Some("-"),
            is_stale: false,
        },
        spinner_frame,
        palette,
    )
}

/// Returns minimal PR detail spans for the preview title.
/// - Pending: "◷ 12m" (dimmed)
/// - Failure: "× lint-check" (danger color)
/// - Success/None: empty
pub fn format_pr_details(
    pr: &PrSummary,
    spinner_frame: u8,
    palette: &ThemePalette,
) -> Vec<ratatui::text::Span<'static>> {
    shared_format_pr_details(pr, spinner_frame, palette)
}

/// Shared git/PR fetch state for resource table headers.
pub(crate) struct ResourceHeaderState<'a> {
    pub palette: &'a ThemePalette,
    pub spinner_frame: u8,
    pub git_fetching: bool,
    pub pr_fetching: bool,
}

/// Build the shared prefix columns for agent and worktree resource tables.
pub(crate) fn resource_table_header(
    state: ResourceHeaderState<'_>,
    show_pr_column: bool,
    trailing_columns: &[&'static str],
) -> Row<'static> {
    let git_header = build_column_header(
        "Git",
        state.git_fetching,
        state.spinner_frame,
        state.palette,
    );
    let header_style = Style::default().fg(state.palette.header).bold();
    let mut header_cells = vec![
        Cell::from("#").style(header_style),
        Cell::from("Project").style(header_style),
        Cell::from("Worktree").style(header_style),
        Cell::from(git_header),
    ];

    if show_pr_column {
        let pr_header =
            build_column_header("PR", state.pr_fetching, state.spinner_frame, state.palette);
        header_cells.push(Cell::from(pr_header));
    }

    for column in trailing_columns {
        header_cells.push(Cell::from(*column).style(header_style));
    }

    Row::new(header_cells).height(1)
}

/// Bordered panel block with dashboard header styling.
pub(crate) fn panel_block(
    title: impl Into<Line<'static>>,
    palette: &ThemePalette,
) -> Block<'static> {
    let title_style = Style::default()
        .fg(palette.header)
        .add_modifier(Modifier::BOLD);
    let border_style = Style::default().fg(palette.border);
    Block::bordered()
        .title(title)
        .title_style(title_style)
        .border_style(border_style)
}

/// Build a column header with optional spinner when data is being fetched.
pub fn build_column_header(
    name: &str,
    is_fetching: bool,
    spinner_frame: u8,
    palette: &ThemePalette,
) -> Line<'static> {
    if is_fetching {
        let frame = SPINNER_FRAMES[spinner_frame as usize % SPINNER_FRAMES.len()];
        Line::from(vec![
            Span::styled(
                format!("{} ", name),
                Style::default().fg(palette.header).bold(),
            ),
            Span::styled(frame.to_string(), Style::default().fg(palette.dimmed)),
        ])
    } else {
        Line::from(Span::styled(
            name.to_string(),
            Style::default().fg(palette.header).bold(),
        ))
    }
}

/// Convert vec of (string, style) pairs into a ratatui Line for table cell rendering.
pub fn spans_to_line(spans: Vec<(String, Style)>) -> Line<'static> {
    Line::from(
        spans
            .into_iter()
            .map(|(text, style)| Span::styled(text, style))
            .collect::<Vec<_>>(),
    )
}

/// Calculate column width from string items with min/max clamping.
pub fn calc_column_width(items: &[String], min: usize, max: usize, padding: usize) -> u16 {
    items
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(min)
        .clamp(min, max)
        .saturating_add(padding) as u16
}

/// Create a style for a table row based on whether it's the current or main worktree.
pub fn make_row_style(is_current: bool, is_main: bool, palette: &ThemePalette) -> Style {
    if is_current {
        Style::default().fg(palette.current_worktree_fg)
    } else if is_main {
        Style::default().fg(palette.dimmed)
    } else {
        Style::default()
    }
}
