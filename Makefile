.PHONY: core dev test lint format doctor docker-up docker-down clean schema schema-check schema-py schema-py-check web-deps web \
        desktop-lock desktop-lock-check desktop-env notices notices-check sidecar desktop geometry-check

PYTHON ?= python
NPM ?= npm

# Rust core binary used by POST /api/v1/core (found automatically under crates/promptcad-core/target).
# The repo-root rust-toolchain.toml pins the Windows MSVC toolchain, which does not exist on
# Linux/macOS, so there the host stable toolchain is selected (as CI does); override freely.
ifneq ($(OS),Windows_NT)
core: export RUSTUP_TOOLCHAIN ?= stable
endif
core:
	cd crates/promptcad-core && cargo build --release --locked --bins

dev:
	uvicorn app.main:app --reload --host 0.0.0.0 --port 8000

test:
	pytest

lint:
	ruff check .

format:
	ruff format .
	ruff check --fix .

doctor:
	python scripts/doctor.py

docker-up:
	docker compose up --build

docker-down:
	docker compose down

clean:
	python -c "from pathlib import Path; import shutil; p=Path('generated'); [shutil.rmtree(x) for x in p.iterdir() if x.is_dir()]"

# JSON Schema exported from app/models (ADR 0008); web/ generates TypeScript from it.
schema:
	$(PYTHON) scripts/export_schema.py

schema-check:
	$(PYTHON) scripts/export_schema.py --check

# Pydantic models generated from schemas/cad2 into app/models/generated (ADR 0008).
# Needs the `schema` extra: pip install -e ".[schema]".
schema-py:
	$(PYTHON) scripts/gen_pydantic.py

schema-py-check:
	$(PYTHON) scripts/gen_pydantic.py --check

web-deps:
	$(NPM) --prefix web ci

# Builds the React frontend into app/web_dist (served at / when present).
web: schema
	$(NPM) --prefix web run gen:types
	$(NPM) --prefix web run build

# Hashed per-platform locks for the sidecar build env (ADR 0011):
# requirements/desktop-{windows-x86_64,macos-arm64,linux-x86_64}.txt. Needs uv.
desktop-lock:
	$(PYTHON) scripts/desktop_lock.py

desktop-lock-check:
	$(PYTHON) scripts/desktop_lock.py --check

# .venv-desktop (cadquery + PyInstaller) installed from this platform's lock file.
desktop-env:
	$(PYTHON) scripts/desktop_env.py

ifeq ($(OS),Windows_NT)
DESKTOP_PYTHON ?= .venv-desktop/Scripts/python.exe
else
DESKTOP_PYTHON ?= .venv-desktop/bin/python
endif

# THIRD_PARTY_NOTICES.md (the committed file is the Windows one; see ADR 0011).
notices:
	$(DESKTOP_PYTHON) scripts/gen_third_party_notices.py --python $(DESKTOP_PYTHON) --strict

notices-check:
	$(DESKTOP_PYTHON) scripts/gen_third_party_notices.py --python $(DESKTOP_PYTHON) --check --strict

# PyInstaller onedir -> src-tauri/binaries/promptcad-sidecar/ (run with .venv-desktop).
sidecar: core
	$(DESKTOP_PYTHON) scripts/build_sidecar.py

# Cad2Compiler vs the frozen v1 geometry golden (tests/fixtures/v1_geometry_golden.json) plus the 2.0-native checks;
# needs the CadQuery venv and the core binary.
geometry-check: core
	$(DESKTOP_PYTHON) scripts/geometry_equivalence.py

# Tauri bundle; beforeBuildCommand in src-tauri/tauri.conf.json builds the frontend.
desktop: sidecar
	$(NPM) run tauri -- build
