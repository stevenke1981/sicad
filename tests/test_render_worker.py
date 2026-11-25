from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from app import render_worker
from app.render_worker import RENDER_ROOT_ENV

ROOT = Path(__file__).resolve().parents[1]

# A cadquery-free stand-in for a generated model.py: records how it was invoked.
FAKE_MODEL = """\
import json
import sys
from pathlib import Path

args = sys.argv[1:]
out = Path(args[args.index('--output-dir') + 1])
(out / 'marker.json').write_text(json.dumps({
    'name': __name__,
    'argv': sys.argv,
    'uvicorn_loaded': 'uvicorn' in sys.modules,
    'fastapi_loaded': 'fastapi' in sys.modules,
}), encoding='utf-8')
"""


@pytest.fixture
def render_root(tmp_path, monkeypatch):
    root = tmp_path / "data"
    root.mkdir()
    monkeypatch.setenv(RENDER_ROOT_ENV, str(root))
    return root


def _job(root: Path, name: str = "job1", body: str = FAKE_MODEL, filename: str = "model.py"):
    job = root / name
    job.mkdir(parents=True)
    model = job / filename
    model.write_text(body, encoding="utf-8")
    return job, model


def _marker(job: Path) -> dict:
    return json.loads((job / "marker.json").read_text(encoding="utf-8"))


def _argv(model: Path, output_dir: Path, *formats: str) -> list[str]:
    return [str(model), "--output-dir", str(output_dir), "--formats", *formats]


def test_valid_job_runs_model_as_main(render_root):
    job, model = _job(render_root)
    saved = list(sys.argv)

    assert render_worker.main(_argv(model, job, "step", "stl")) == 0

    marker = _marker(job)
    assert marker["name"] == "__main__"
    assert marker["argv"] == [
        str(model.resolve()),
        "--output-dir",
        str(job.resolve()),
        "--formats",
        "step",
        "stl",
    ]
    assert sys.argv == saved


def test_job_outside_root_is_rejected(render_root, tmp_path, capsys):
    job, model = _job(tmp_path / "elsewhere")

    assert render_worker.main(_argv(model, job, "step")) == 2
    assert not (job / "marker.json").exists()
    assert "rejected" in capsys.readouterr().err


def test_root_itself_is_not_a_job_dir(render_root):
    model = render_root / "model.py"
    model.write_text(FAKE_MODEL, encoding="utf-8")

    assert render_worker.main(_argv(model, render_root, "step")) == 2
    assert not (render_root / "marker.json").exists()


def test_wrong_filename_is_rejected(render_root):
    job, model = _job(render_root, filename="evil.py")

    assert render_worker.main(_argv(model, job, "step")) == 2
    assert not (job / "marker.json").exists()


def test_output_dir_must_be_model_parent(render_root):
    job, model = _job(render_root, "job1")
    other = render_root / "job2"
    other.mkdir()

    assert render_worker.main(_argv(model, other, "step")) == 2
    assert render_worker.main(_argv(model, render_root, "step")) == 2
    assert not (job / "marker.json").exists()
    assert not (other / "marker.json").exists()


def test_missing_root_env_is_rejected(render_root, monkeypatch):
    job, model = _job(render_root)
    monkeypatch.delenv(RENDER_ROOT_ENV)

    assert render_worker.main(_argv(model, job, "step")) == 2
    assert not (job / "marker.json").exists()


def test_nonexistent_root_is_rejected(render_root, monkeypatch, tmp_path):
    job, model = _job(render_root)
    monkeypatch.setenv(RENDER_ROOT_ENV, str(tmp_path / "missing"))

    assert render_worker.main(_argv(model, job, "step")) == 2
    assert not (job / "marker.json").exists()


def test_dotdot_traversal_out_of_root_is_rejected(render_root, tmp_path):
    outside, _ = _job(tmp_path / "outside", "job")
    escaped = render_root / ".." / "outside" / "job"

    assert render_worker.main(_argv(escaped / "model.py", escaped, "step")) == 2
    assert not (outside / "marker.json").exists()


def test_relative_paths_are_rejected(render_root, monkeypatch):
    job, _ = _job(render_root)
    monkeypatch.chdir(job)

    assert render_worker.main(["model.py", "--output-dir", ".", "--formats", "step"]) == 2
    assert not (job / "marker.json").exists()


def test_unknown_format_is_a_usage_error(render_root):
    job, model = _job(render_root)

    assert render_worker.main(_argv(model, job, "obj")) == 2
    assert not (job / "marker.json").exists()


def test_model_exception_exits_1_with_traceback(render_root, capsys):
    job, model = _job(render_root, body="raise ValueError('boom from model')\n")

    assert render_worker.main(_argv(model, job, "step")) == 1
    err = capsys.readouterr().err
    assert "Traceback" in err
    assert "boom from model" in err


@pytest.mark.parametrize(
    ("body", "expected"),
    [
        ("raise SystemExit(0)\n", 0),
        ("raise SystemExit(None)\n", 0),
        ("raise SystemExit(3)\n", 1),
        ("raise SystemExit('fatal message')\n", 1),
    ],
)
def test_model_system_exit_is_mapped(render_root, body, expected):
    job, model = _job(render_root, body=body)

    assert render_worker.main(_argv(model, job, "step")) == expected


def test_sidecar_dispatches_render_worker_without_starting_server(tmp_path):
    root = tmp_path / "data"
    job, model = _job(root)
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("PROMPTCAD_")}
    env.update({RENDER_ROOT_ENV: str(root), "PYTHONPATH": str(ROOT)})

    completed = subprocess.run(
        [sys.executable, "-m", "app.sidecar", "--render-worker", *_argv(model, job, "svg")],
        cwd=job,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )

    assert completed.returncode == 0, completed.stderr
    assert "PROMPTCAD_SIDECAR_READY" not in completed.stdout
    marker = _marker(job)
    assert marker["name"] == "__main__"
    assert marker["argv"][-2:] == ["--formats", "svg"]
    assert marker["uvicorn_loaded"] is False
    assert marker["fastapi_loaded"] is False


def test_sidecar_render_worker_rejection_exits_2(tmp_path):
    job, model = _job(tmp_path / "data")
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("PROMPTCAD_")}
    env["PYTHONPATH"] = str(ROOT)

    completed = subprocess.run(
        [sys.executable, "-m", "app.sidecar", "--render-worker", *_argv(model, job, "step")],
        cwd=job,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )

    assert completed.returncode == 2
    assert RENDER_ROOT_ENV in completed.stderr
    assert not (job / "marker.json").exists()
