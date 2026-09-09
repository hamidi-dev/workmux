"""Regression tests for command execution and identity in the test harness."""

from pathlib import Path

import pytest

from .conftest import (
    TmuxEnvironment,
    WezTermEnvironment,
    make_env_script,
    poll_until,
)


def test_env_script_runs_through_interpreter(tmp_path: Path):
    """Generated commands read scripts, without requiring shebang execution."""
    env = TmuxEnvironment(tmp_path)
    scripts = tmp_path / "scripts with 'quotes'"
    scripts.mkdir()
    env._scripts_dir = scripts
    value = "spaces, 'quotes', and $literal"
    command = make_env_script(
        env, 'printf "%s" "$HARNESS_VALUE"; exit 17', {"HARNESS_VALUE": value}
    )
    script = next(scripts.iterdir())
    script.chmod(0o644)

    result = env.run_command(["/bin/sh", "-c", command], check=False)

    assert result.returncode == 17
    assert result.stdout == value
    assert result.stderr == ""
    assert "HARNESS_VALUE" not in env.env


@pytest.mark.tmux_only
def test_window_identity_survives_automatic_rename(mux_server: TmuxEnvironment):
    """A foreground process can change a name without replacing the window."""
    env = mux_server
    before = env.list_window_ids()
    runner = before[0]
    assert (
        env.tmux(
            ["show-options", "-Avw", "-t", runner, "automatic-rename"]
        ).stdout.strip()
        == "on"
    )

    env.send_keys(runner, "exec sleep 30")
    assert poll_until(lambda: env.list_windows() == ["sleep"])
    assert env.list_window_ids() == before

    env.new_window("other")
    assert env.list_window_ids() != before
    env.select_window(runner)
    assert env.tmux(["display-message", "-p", "#{window_id}"]).stdout.strip() == runner

    env.kill_window(runner)
    assert runner not in env.list_window_ids()


def test_wezterm_window_ids_distinguish_tabs_not_titles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    env = WezTermEnvironment(tmp_path)
    monkeypatch.setattr(
        env,
        "_list_panes",
        lambda: [
            {"tab_id": 7, "tab_title": "sh"},
            {"tab_id": 7, "tab_title": "sh"},
            {"tab_id": 8, "tab_title": "sh"},
        ],
    )
    assert env.list_window_ids() == ["7", "8"]
