.PHONY: dev test lint format doctor docker-up docker-down clean schema schema-check web-deps web sidecar desktop

PYTHON ?= python
NPM ?= npm

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

web-deps:
	$(NPM) --prefix web ci

# Builds the React frontend into app/web_dist (served at / when present).
web: schema
	$(NPM) --prefix web run gen:types
	$(NPM) --prefix web run build

# PyInstaller onefile -> src-tauri/binaries/promptcad-sidecar-<target-triple>[.exe]
# (needs the desktop extra: pip install -e ".[desktop]").
sidecar:
	$(PYTHON) scripts/build_sidecar.py

# Tauri bundle; beforeBuildCommand in src-tauri/tauri.conf.json builds the frontend.
desktop: sidecar
	$(NPM) run tauri -- build
