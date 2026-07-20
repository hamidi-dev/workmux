"""Integration tests for sidebar-aware tmux layouts."""

from pathlib import Path
from typing import cast

import pytest

from .conftest import MuxEnvironment, TmuxEnvironment, run_workmux_command


pytestmark = pytest.mark.tmux_only


def _pane_rows(env: TmuxEnvironment, target: str) -> list[dict[str, str]]:
    output = env.tmux(
        [
            "list-panes",
            "-t",
            target,
            "-F",
            "#{pane_id}\t#{@workmux_role}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}\t#{pane_active}",
        ]
    ).stdout
    keys = ["id", "role", "left", "top", "width", "height", "active"]
    return [dict(zip(keys, line.split("\t"))) for line in output.splitlines()]


def _add_content_panes(env: TmuxEnvironment, target: str, count: int = 2) -> None:
    for _ in range(count):
        env.tmux(["split-window", "-d", "-t", target, "sleep 60"])


def _add_sidebar(
    env: TmuxEnvironment, target: str, position: str = "left", size: int = 12
) -> str:
    split_flag = "-hbf" if position == "left" else "-vbf"
    pane_id = env.tmux(
        [
            "split-window",
            split_flag,
            "-d",
            "-l",
            str(size),
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            target,
            "sleep 60",
        ]
    ).stdout.strip()
    env.tmux(["set-option", "-p", "-t", pane_id, "@workmux_role", "sidebar"])
    env.tmux(["set-option", "-g", "@workmux_sidebar_position", position])
    size_option = (
        "@workmux_sidebar_width" if position == "left" else "@workmux_sidebar_height"
    )
    env.tmux(["set-option", "-g", size_option, str(size)])
    return pane_id


def _run_layout(
    env: TmuxEnvironment,
    workmux_exe_path: Path,
    repo_path: Path,
    arguments: str,
    *,
    expect_fail: bool = False,
):
    return run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"sidebar layout {arguments}".strip(),
        expect_fail=expect_fail,
    )


def test_even_vertical_excludes_left_sidebar_and_preserves_focus(
    mux_server: MuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    env = cast(TmuxEnvironment, mux_server)
    _add_content_panes(env, "test:")
    sidebar_id = _add_sidebar(env, "test:")
    active_before = next(
        row["id"] for row in _pane_rows(env, "test:") if row["active"] == "1"
    )

    _run_layout(env, workmux_exe_path, repo_path, "even-vertical")

    rows = _pane_rows(env, "test:")
    sidebar = next(row for row in rows if row["role"] == "sidebar")
    content = [row for row in rows if row["role"] != "sidebar"]
    assert len(rows) == 4
    assert sidebar["id"] == sidebar_id
    assert sidebar["left"] == "0"
    assert sidebar["width"] == "12"
    assert len({row["left"] for row in content}) == 1
    assert len({row["width"] for row in content}) == 1
    assert len({row["top"] for row in content}) == len(content)
    assert next(row["id"] for row in rows if row["active"] == "1") == active_before


def test_next_cycles_content_layout_without_leaking_windows(
    mux_server: MuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    env = cast(TmuxEnvironment, mux_server)
    _add_content_panes(env, "test:")
    _add_sidebar(env, "test:")
    windows_before = env.tmux(
        ["list-windows", "-F", "#{window_id}"]
    ).stdout.splitlines()

    _run_layout(env, workmux_exe_path, repo_path, "even-horizontal")
    horizontal = [row for row in _pane_rows(env, "test:") if row["role"] != "sidebar"]
    assert len({row["top"] for row in horizontal}) == 1
    assert len({row["left"] for row in horizontal}) == len(horizontal)

    _run_layout(env, workmux_exe_path, repo_path, "")
    vertical = [row for row in _pane_rows(env, "test:") if row["role"] != "sidebar"]
    assert len({row["left"] for row in vertical}) == 1
    assert len({row["top"] for row in vertical}) == len(vertical)
    assert (
        env.tmux(["list-windows", "-F", "#{window_id}"]).stdout.splitlines()
        == windows_before
    )


def test_top_sidebar_layout_can_target_an_inactive_window(
    mux_server: MuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    env = cast(TmuxEnvironment, mux_server)
    window_id = env.tmux(
        ["new-window", "-d", "-P", "-F", "#{window_id}", "-n", "layout-target"]
    ).stdout.strip()
    _add_content_panes(env, window_id)
    sidebar_id = _add_sidebar(env, window_id, position="top", size=3)
    current_before = env.tmux(["display-message", "-p", "#{window_id}"]).stdout.strip()

    _run_layout(
        env,
        workmux_exe_path,
        repo_path,
        f"even-horizontal --target {window_id}",
    )

    rows = _pane_rows(env, window_id)
    sidebar = next(row for row in rows if row["role"] == "sidebar")
    content = [row for row in rows if row["role"] != "sidebar"]
    assert sidebar["id"] == sidebar_id
    assert sidebar["top"] == "0"
    assert sidebar["height"] == "3"
    assert len({row["top"] for row in content}) == 1
    assert len({row["left"] for row in content}) == len(content)
    assert (
        env.tmux(["display-message", "-p", "#{window_id}"]).stdout.strip()
        == current_before
    )


def test_invalid_layout_restores_sidebar_and_removes_temporary_window(
    mux_server: MuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    env = cast(TmuxEnvironment, mux_server)
    _add_content_panes(env, "test:")
    sidebar_id = _add_sidebar(env, "test:")
    windows_before = env.tmux(
        ["list-windows", "-F", "#{window_id}"]
    ).stdout.splitlines()

    result = _run_layout(
        env,
        workmux_exe_path,
        repo_path,
        "not-a-real-layout",
        expect_fail=True,
    )

    rows = _pane_rows(env, "test:")
    assert result.exit_code != 0
    assert next(row for row in rows if row["role"] == "sidebar")["id"] == sidebar_id
    assert len(rows) == 4
    assert (
        env.tmux(["list-windows", "-F", "#{window_id}"]).stdout.splitlines()
        == windows_before
    )


def test_layout_delegates_when_sidebar_is_absent(
    mux_server: MuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    env = cast(TmuxEnvironment, mux_server)
    _add_content_panes(env, "test:")

    _run_layout(env, workmux_exe_path, repo_path, "even-vertical")

    rows = _pane_rows(env, "test:")
    assert len(rows) == 3
    assert len({row["left"] for row in rows}) == 1
    assert len({row["top"] for row in rows}) == len(rows)
