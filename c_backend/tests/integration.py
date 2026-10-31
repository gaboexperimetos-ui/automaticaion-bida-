#!/usr/bin/env python3
"""Pruebas HTTP reales del backend C, sin Rust ni servicios externos."""

from __future__ import annotations

import concurrent.futures
import datetime as dt
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get("SP_C_BIN", ROOT / "c_backend/build/study-platform-c")).resolve()
CHECKS = 0


def check(condition: bool, message: str) -> None:
    global CHECKS
    CHECKS += 1
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def request(port: int, path: str, method: str = "GET") -> tuple[int, dict[str, str], bytes]:
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method)
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            return response.status, dict(response.headers.items()), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers.items()), error.read()


def raw_request(port: int, payload: bytes) -> bytes:
    with socket.create_connection(("127.0.0.1", port), timeout=5) as sock:
        sock.settimeout(5)
        sock.sendall(payload)
        chunks = []
        while True:
            try:
                chunk = sock.recv(4096)
            except socket.timeout:
                break
            if not chunk:
                break
            chunks.append(chunk)
        return b"".join(chunks)


def wait_ready(process: subprocess.Popen[str], port: int) -> None:
    deadline = time.monotonic() + 15
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            stdout, stderr = process.communicate()
            raise RuntimeError(f"El servidor terminó antes de arrancar ({process.returncode})\n{stdout}\n{stderr}")
        try:
            status, _, _ = request(port, "/api/config")
            if status == 200:
                return
        except Exception as error:  # el socket todavía puede estar cerrado
            last_error = error
        time.sleep(0.05)
    raise RuntimeError(f"Timeout esperando al servidor: {last_error}")


def main() -> None:
    check(BINARY.is_file(), f"No existe el binario {BINARY}")
    port = free_port()
    with tempfile.TemporaryDirectory(prefix="study-platform-c-") as temp:
        env = os.environ.copy()
        for key in list(env):
            if key.startswith("OPENROUTER") or key in {
                "CUSTOM_MCP_API_KEY", "REGISTRATION_TOKEN", "AUTH_DISABLED"
            }:
                env.pop(key, None)
        env.update({
            "SESSION_SECRET": "integration-secret-0123456789abcdef-0123456789",
            "HOST": "127.0.0.1",
            "PORT": str(port),
            "DB_PATH": str(Path(temp) / "data/app.db"),
            "NOTES_DIR": str(Path(temp) / "notes"),
            "MIKARENO_MCP_URL": "http://127.0.0.1:9/mcp",
            "GENOFFICE_MCP_URL": "http://127.0.0.1:9/mcp",
            "SP_STATIC_ROOT": str(ROOT),
            "C_WORKERS": "4",
        })
        process = subprocess.Popen(
            [str(BINARY), "--addr", f"127.0.0.1:{port}"],
            cwd=temp,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            wait_ready(process, port)
            status, headers, body = request(port, "/health")
            health = json.loads(body)
            check(status == 200, f"/health devolvió {status}: {body!r}")
            check(health["status"] == "healthy", "SQLite no está healthy")
            check(health["components"]["database"] is True, "database != true")
            check(health["components"]["openrouter"] is False, "Se anunció OpenRouter sin clave")
            check(isinstance(health["components"]["mikareno_mcp"], bool), "Mikareno no es bool")
            check(isinstance(health["components"]["genoffice_mcp"], bool), "GenOffice no es bool")
            dt.datetime.fromisoformat(health["timestamp"].replace("Z", "+00:00"))
            check(health["version"] == "0.1.0", "Versión incorrecta")
            check(headers.get("X-Content-Type-Options") == "nosniff", "Falta nosniff")

            status, _, config_body = request(port, "/api/config?source=test")
            config = json.loads(config_body)
            check(status == 200, "Falló /api/config")
            check(config["openrouter_model"] == "cohere/north-mini-code:free", "Modelo por defecto distinto")
            check(config["is_localhost"] is True, "is_localhost incorrecto")
            check(b"integration-secret" not in config_body, "Se filtró SESSION_SECRET")
            status_alias, _, alias_body = request(port, "/api/public-config")
            check(status_alias == 200 and alias_body == config_body, "El alias de config no es idéntico")

            assets = {
                "/shell.js": ("static/app/shell.js", "no-cache"),
                "/nova.css": ("static/app/nova.css", "no-cache"),
                "/polish.css": ("static/app/polish.css", "no-cache"),
                "/legacy": ("static/study-platform-ui/index.html", None),
                "/styles.css": ("static/study-platform-ui/styles.css", None),
                "/app.js": ("static/study-platform-ui/app.js", None),
                "/favicon.svg": ("static/app/favicon.svg", None),
                "/favicon.ico": ("static/app/favicon.svg", None),
            }
            for route, (relative, cache) in assets.items():
                status, headers, content = request(port, route)
                check(status == 200, f"{route}: HTTP {status}")
                check(content == (ROOT / relative).read_bytes(), f"{route}: contenido distinto")
                check(headers.get("Cache-Control") == cache, f"{route}: caché incorrecta")

            status, _, _ = request(port, "/no-existe")
            check(status == 404, "Ruta desconocida no devolvió 404")
            status, _, _ = request(port, "/health", method="POST")
            check(status == 405, "POST /health no devolvió 405")
            malformed = raw_request(port, b"BAD REQUEST\r\n\r\n")
            check(malformed.startswith(b"HTTP/1.1 400 "), "Petición malformada no devolvió 400")
            transfer = raw_request(
                port,
                b"POST /health HTTP/1.1\r\nTransfer-Encoding: chunked\r\nHost: local\r\n\r\n0\r\n\r\n",
            )
            check(transfer.startswith(b"HTTP/1.1 501 "), "Transfer-Encoding no falla cerrado")
            oversized_headers = raw_request(
                port, b"GET /health HTTP/1.1\r\nX-Relleno: " + b"x" * 17000 + b"\r\n\r\n"
            )
            check(oversized_headers.startswith(b"HTTP/1.1 431 "), "No se limitó el tamaño de headers")
            oversized_body = raw_request(
                port,
                b"POST /health HTTP/1.1\r\nContent-Length: 1048577\r\nHost: local\r\n\r\n",
            )
            check(oversized_body.startswith(b"HTTP/1.1 413 "), "No se limitó el tamaño del body")
            conflicting_lengths = raw_request(
                port,
                b"POST /health HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            )
            check(conflicting_lengths.startswith(b"HTTP/1.1 400 "), "Se aceptaron Content-Length contradictorios")
            body_first_header = raw_request(
                port,
                b"POST /health HTTP/1.1\r\nContent-Length: 4\r\nHost: local\r\n\r\ntest",
            )
            check(body_first_header.startswith(b"HTTP/1.1 405 "), "No se leyó el body declarado en el primer header")

            with concurrent.futures.ThreadPoolExecutor(max_workers=12) as executor:
                results = list(executor.map(lambda _: request(port, "/api/public-config"), range(24)))
            check(all(item[0] == 200 and item[2] == config_body for item in results),
                  "Fallaron peticiones concurrentes")
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
            try:
                stdout, stderr = process.communicate(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                stdout, stderr = process.communicate(timeout=5)
            check(process.returncode == 0,
                  f"El servidor no cerró limpiamente ({process.returncode})\n{stdout}\n{stderr}")
            if process.returncode != 0:
                print(stdout)
                print(stderr)
    print(f"integration.py: {CHECKS} comprobaciones HTTP en verde")


if __name__ == "__main__":
    main()
