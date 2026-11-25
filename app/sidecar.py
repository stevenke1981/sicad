"""Desktop sidecar entry point (ADR 0002).

The Tauri shell spawns this process (``python -m app.sidecar`` in development, or the
PyInstaller-frozen ``promptcad-sidecar`` binary) with::

    promptcad-sidecar --host 127.0.0.1 --port 0

and these environment variables (all read through :class:`app.core.config.Settings`):

* ``PROMPTCAD_API_TOKEN``    - one-time bearer token; required unless ``--allow-no-token``.
* ``PROMPTCAD_DATA_DIR``     - where jobs and artifacts are written (the app data dir).
* ``PROMPTCAD_CORS_ORIGINS`` - comma separated, e.g.
  ``http://tauri.localhost,tauri://localhost``.

The process binds the socket itself (port 0 = let the OS pick a free port), starts
uvicorn on it and, once the server accepts connections, writes exactly one flushed line
to stdout::

    PROMPTCAD_SIDECAR_READY {"port": 51234}

Everything else (uvicorn logs, errors) goes to stderr so stdout stays machine readable.
"""

from __future__ import annotations

import argparse
import copy
import ipaddress
import json
import socket
import sys
from collections.abc import Sequence

READY_PREFIX = "PROMPTCAD_SIDECAR_READY"


def ready_line(port: int) -> str:
    return f"{READY_PREFIX} {json.dumps({'port': port})}"


def _is_loopback(host: str) -> bool:
    if host == "localhost":
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def _bind_socket(host: str, port: int) -> socket.socket:
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    sock = socket.socket(family, socket.SOCK_STREAM)
    if sys.platform == "win32":
        # Prevent another process from binding the same port (no SO_REUSEADDR on Windows).
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
    else:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((host, port))
    return sock


def _stderr_log_config() -> dict:
    """uvicorn's default logging config, with every handler pointed at stderr."""
    from uvicorn.config import LOGGING_CONFIG

    config = copy.deepcopy(LOGGING_CONFIG)
    for handler in config.get("handlers", {}).values():
        if "stream" in handler:
            handler["stream"] = "ext://sys.stderr"
    return config


def _parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="promptcad-sidecar", description="PromptCAD Studio desktop sidecar"
    )
    parser.add_argument("--host", default="127.0.0.1", help="loopback address to bind")
    parser.add_argument("--port", type=int, default=0, help="0 picks a free port")
    parser.add_argument(
        "--log-level",
        default="warning",
        choices=["critical", "error", "warning", "info", "debug", "trace"],
    )
    parser.add_argument(
        "--allow-no-token",
        action="store_true",
        help="development only: start even when PROMPTCAD_API_TOKEN is unset",
    )
    args = parser.parse_args(argv)
    if not 0 <= args.port <= 65535:
        parser.error("--port must be between 0 and 65535")
    if not _is_loopback(args.host):
        parser.error("the sidecar only binds loopback addresses (ADR 0002)")
    return args


def main(argv: Sequence[str] | None = None) -> int:
    args = _parse_args(argv)

    import uvicorn

    from app.core.config import Settings
    from app.main import create_app

    try:
        sock = _bind_socket(args.host, args.port)
    except OSError as exc:
        print(f"promptcad-sidecar: cannot bind {args.host}:{args.port}: {exc}", file=sys.stderr)
        return 2
    port = sock.getsockname()[1]

    settings = Settings(host=args.host, port=port)
    if not settings.api_token and not args.allow_no_token:
        sock.close()
        print(
            "promptcad-sidecar: PROMPTCAD_API_TOKEN must be set (or pass --allow-no-token)",
            file=sys.stderr,
        )
        return 2

    class _SidecarServer(uvicorn.Server):
        async def startup(self, sockets: list[socket.socket] | None = None) -> None:
            await super().startup(sockets=sockets)
            if self.started and not self.should_exit:
                sys.stdout.write(ready_line(port) + "\n")
                sys.stdout.flush()

    config = uvicorn.Config(
        create_app(settings),
        log_config=_stderr_log_config(),
        log_level=args.log_level,
        access_log=False,
        lifespan="on",
        server_header=False,
    )
    server = _SidecarServer(config)
    try:
        server.run(sockets=[sock])
    finally:
        sock.close()
    return 0 if server.started else 1


if __name__ == "__main__":
    raise SystemExit(main())
