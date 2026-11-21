#!/usr/bin/env python3
"""Black-box smoke tests for the Zig HTTP server (Python stdlib only)."""

from __future__ import annotations

import argparse
import json
import http.server
import os
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def request(base: str, path: str, method: str = "GET", payload=None, headers=None):
    request_headers = {"Accept": "application/json"}
    request_headers.update(headers or {})
    body = None
    if payload is not None:
        if isinstance(payload, bytes):
            body = payload
        elif request_headers.get("Content-Type", "").startswith("application/x-www-form-urlencoded"):
            body = urllib.parse.urlencode(payload).encode()
        else:
            body = json.dumps(payload, ensure_ascii=False).encode()
            request_headers.setdefault("Content-Type", "application/json")
    req = urllib.request.Request(base + path, data=body, headers=request_headers, method=method)
    try:
        with OPENER.open(req, timeout=8) as response:
            return response.status, response.headers, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.headers, error.read()


def json_body(response):
    return json.loads(response[2].decode("utf-8"))


class Server:
    def __init__(self, binary: Path, workspace: Path, data_dir: Path, extra_env=None):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}"
        env = os.environ.copy()
        env.update(
            {
                "SESSION_SECRET": "zig-smoke-secret-0123456789abcdef-0123456789",
                "DB_PATH": str(data_dir / "app.db"),
                "NOTES_DIR": str(data_dir / "notes"),
                "AUTH_DISABLED": "true",
                "OPENROUTER_API_KEY": "",
                "CUSTOM_MCP_API_KEY": "smoke-mcp-key",
            }
        )
        env.update(extra_env or {})
        self.process = subprocess.Popen(
            [str(binary), "--addr", f"127.0.0.1:{self.port}"],
            cwd=workspace,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

    def wait_ready(self):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                output = self.process.stdout.read() if self.process.stdout else ""
                raise AssertionError(f"server exited during startup:\n{output}")
            try:
                status, _, body = request(self.base, "/health")
                if status == 200 and json.loads(body).get("status") == "healthy":
                    return
            except (OSError, urllib.error.URLError, json.JSONDecodeError):
                pass
            time.sleep(0.05)
        raise AssertionError("server did not become ready within 12 seconds")

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)


def run_public_smoke(binary: Path, workspace: Path, temp: Path):
    server = Server(binary, workspace, temp / "public")
    try:
        server.wait_ready()
        base = server.base

        status, _, body = request(base, "/health")
        assert status == 200 and json.loads(body)["database"] is True
        status, _, body = request(base, "/api/config")
        public_config = json.loads(body)
        assert status == 200 and "SESSION_SECRET" not in body.decode()
        assert public_config["openrouter_configured"] is False
        status, _, body = request(base, "/admin/metrics")
        metrics = json.loads(body)
        assert status == 200 and metrics["ram_mb_total"] > 0 and metrics["db_bytes"] > 0
        assert metrics["ram_mb_used"] >= 0 and metrics["disk_mb_free"] >= 0

        status, _, html = request(base, "/dashboard", headers={"Accept": "text/html"})
        assert status == 200 and b"<main" in html and b"NOVA" in html
        status, _, office_html = request(base, "/office", headers={"Accept": "text/html"})
        assert status == 200 and b"gen-office-home" in office_html and b"NOVA Office" in office_html
        status, headers, css = request(base, "/static/app/nova.css")
        assert status == 200 and headers.get_content_type() == "text/css" and len(css) > 100_000
        # Regresion: los estaticos no gastan el cubo general (antes 80 GET
        # seguidos acababan en 429 y una pagina normal se rompia al navegar).
        for _ in range(80):
            status, _, _ = request(base, "/static/app/nova.css")
            assert status == 200
        status, headers, htmx = request(base, "/vendor/htmx-2.0.3.min.js")
        assert status == 200 and headers.get_content_type() == "application/javascript" and b"htmx" in htmx[:2048]
        status, headers, ics_js = request(base, "/calendar-ics.js")
        assert status == 200 and headers.get_content_type() == "application/javascript" and b"parseIcs" in ics_js
        status, headers, xlsx_js = request(base, "/vendor/xlsx-0.20.3.full.min.js")
        assert status == 200 and headers.get_content_type() == "application/javascript" and len(xlsx_js) > 100_000

        status, _, body = request(
            base,
            "/api/notes",
            "POST",
            {"title": "Zig Smoke Note", "content": "first version: quasar and polynomials", "tags": "ci, study"},
        )
        assert status == 201
        slug = json_body((status, None, body))["slug"]
        note_path = "/api/notes/" + urllib.parse.quote(slug, safe="")
        status, _, body = request(base, note_path)
        assert status == 200 and "quasar" in json_body((status, None, body))["content"]

        status, _, body = request(base, "/api/search?q=quasar")
        search = json.loads(body)
        assert status == 200 and search["count"] == 1 and search["results"][0]["slug"] == slug
        status, _, body = request(base, "/dashboard/search?q=quasar", headers={"Accept": "text/html"})
        assert status == 200 and slug.encode() in body

        status, _, _ = request(
            base,
            note_path,
            "PUT",
            {"content": "second version: nebula and revised algebra", "tags": "ci, updated"},
        )
        assert status == 200
        status, _, body = request(base, note_path + "/versions")
        versions = json.loads(body)["versions"]
        assert status == 200 and len(versions) == 2
        status, headers, body = request(base, "/api/notes/" + urllib.parse.quote(slug, safe="") + "/export")
        assert status == 200 and headers.get_content_type() == "text/markdown" and b"revised algebra" in body

        old_version_id = versions[-1]["id"]
        status, headers, _ = request(base, f"/app/notes/{urllib.parse.quote(slug, safe='')}/versions/{old_version_id}/restore", "POST")
        assert status == 303 and headers.get("Location") == f"/app/notes/{slug}"
        status, _, body = request(base, note_path)
        assert status == 200 and "quasar" in json.loads(body)["content"]

        status, _, body = request(
            base,
            "/api/calendar/annotations",
            "POST",
            {"date": "2024-02-29", "title": "Repaso; álgebra", "body": "Primera línea,\nsegunda línea"},
        )
        assert status == 201
        annotation_id = json_body((status, None, body))["id"]
        status, _, body = request(base, "/dashboard/calendar.ics")
        calendar = body.decode("utf-8")
        assert status == 200 and "DTSTART;VALUE=DATE:20240229" in calendar
        assert "DTEND;VALUE=DATE:20240301" in calendar and r"SUMMARY:Repaso\; álgebra" in calendar
        assert r"DESCRIPTION:Primera línea\,\nsegunda línea" in calendar
        status, _, body = request(
            base,
            f"/api/calendar/annotations/{annotation_id}",
            "PUT",
            {"date": "2026-11-13", "title": "Examen", "body": "Actualizado"},
        )
        assert status == 200 and json.loads(body)["date"] == "2026-11-13"
        status, _, body = request(base, "/api/calendar/annotations?from=2026-11-01&to=2026-11-30")
        assert status == 200 and len(json.loads(body)["annotations"]) == 1
        status, _, _ = request(base, "/api/calendar/annotations", "POST", {"date": "2026-02-30", "title": "bad"})
        assert status == 400

        office_form = {"Content-Type": "application/x-www-form-urlencoded", "Accept": "text/html"}
        status, headers, _ = request(base, "/office/docs", "POST", {"title": "Smoke Document", "content": "# Office Preview\n\nThis is **bold** text."}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/docs/smoke-document?saved=1"
        status, _, body = request(base, "/office/docs/smoke-document/preview", headers={"Accept": "text/html"})
        assert status == 200 and b"<h1>Office Preview</h1>" in body and b"<strong>bold</strong>" in body
        status, headers, body = request(base, "/office/docs/smoke-document/export")
        assert status == 200 and headers.get_content_type() == "text/markdown" and b"Office Preview" in body
        status, headers, _ = request(base, "/office/sheets", "POST", {"title": "Formula Sheet"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/sheets/formula-sheet"
        status, headers, _ = request(base, "/office/sheets/formula-sheet/import", "POST", {"csv_data": "1,=SUM(A1:A3)\n2,\n3,\n"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/sheets/formula-sheet"
        status, headers, body = request(base, "/office/sheets/formula-sheet/export")
        assert status == 200 and headers.get_content_type() == "text/csv" and b"6" in body
        status, headers, _ = request(base, "/office/presentations", "POST", {"title": "Smoke Deck"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/smoke-deck"
        status, headers, _ = request(base, "/office/presentations/smoke-deck/slides", "POST", {"title": "Slide Title", "content": "# Slide Content\n\n- Bullet One"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/smoke-deck"
        status, _, body = request(base, "/office/presentations/smoke-deck/present", headers={"Accept": "text/html"})
        assert status == 200 and b"<h1>Slide Content</h1>" in body and b"genPresentationTotal = 1" in body
        status, _, body = request(base, "/office/library")
        assert status == 200 and b"smoke-document" in body and b"formula-sheet" in body and b"smoke-deck" in body
        status, headers, body = request(base, "/office/library/docs/smoke-document/download")
        assert status == 200 and headers.get_content_type() == "text/markdown" and b"Office Preview" in body
        status, _, body = request(base, "/office/library/search?q=deck")
        assert status == 200 and b"smoke-deck" in body
        status, _, body = request(base, "/office/report")
        assert status == 503 and json.loads(body)["error"] == "GenOffice no disponible"

        status, _, body = request(base, "/api/ai/cost")
        assert status == 200 and "budget_limit" in json.loads(body)
        status, _, body = request(base, "/api/ai/model")
        assert status == 200 and json.loads(body)["model"] == "cohere/north-mini-code:free"
        status, _, body = request(base, "/api/ai/model", "PUT", {"model": "openai/gpt-4o-mini"})
        assert status == 200 and json.loads(body)["model"] == "openai/gpt-4o-mini"
        status, _, body = request(base, "/api/ai/chat", "POST", {"message": "Explain quasar", "subject": "smoke-math"})
        assert status == 200 and "Modo simulado" in json.loads(body)["content"]
        status, _, body = request(base, "/api/ai/conversations/smoke-math/messages")
        assert status == 200 and len(json.loads(body)["messages"]) == 2
        status, _, body = request(base, "/api/ai/conversations")
        assert status == 200 and any(c["subject"] == "smoke-math" for c in json.loads(body)["conversations"])
        status, headers, body = request(base, "/api/ai/chat/stream", "POST", {"message": "Explain quasar"})
        assert status == 200 and headers.get_content_type() == "text/event-stream"
        assert b"event: delta" in body and b"event: done" in body and headers.get("x-chat-stream-id")
        status, _, body = request(base, "/api/ai/summary", "POST", {"content": "Study material"})
        assert status == 200 and "summary" in json.loads(body)

        mcp_headers = {"X-API-Key": "smoke-mcp-key"}
        status, _, body = request(base, "/mcp", "POST", {"jsonrpc": "2.0", "id": 1, "method": "initialize"}, mcp_headers)
        assert status == 200 and json.loads(body)["result"]["protocolVersion"] == "2025-03-26"
        status, _, body = request(base, "/mcp", "POST", {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, mcp_headers)
        tools = json.loads(body)["result"]["tools"]
        assert status == 200 and len(tools) == 9
        status, _, _ = request(base, "/mcp", "POST", {"jsonrpc": "2.0", "id": 3, "method": "tools/list"})
        assert status == 401

        status, _, body = request(base, "/api/admin/backup", "POST")
        backup = json.loads(body)
        assert status == 200 and backup["success"] is True and backup["bytes"] > 0
        backup_name = backup["name"]
        sidecar_path = Path(temp) / "public" / "backups" / f"{backup_name}.sha256"
        assert sidecar_path.is_file() and len(sidecar_path.read_text().split()[0]) == 64
        status, _, body = request(base, "/api/admin/backup/restore", "POST", {"name": backup_name})
        assert status == 200 and json.loads(body)["success"] is True
        sidecar_content = sidecar_path.read_text(encoding="utf-8")
        tampered = ("0" if sidecar_content[0] != "0" else "1") + sidecar_content[1:]
        sidecar_path.write_text(tampered, encoding="utf-8")
        status, _, body = request(base, "/api/admin/backup/restore", "POST", {"name": backup_name})
        assert status == 422 and "SHA-256" in json.loads(body)["error"]
        sidecar_path.write_text(sidecar_content, encoding="utf-8")
        status, headers, body = request(base, f"/api/admin/backup/download/{backup_name}")
        assert status == 200 and headers.get_content_type() == "application/vnd.sqlite3" and body.startswith(b"SQLite format 3\x00")
        status, _, body = request(base, "/api/notes", "POST", {"title": "After Backup", "content": "must disappear on restore"})
        assert status == 201
        status, _, body = request(base, "/api/admin/backup/restore", "POST", {"name": backup_name})
        assert status == 200 and json.loads(body)["success"] is True
        status, _, body = request(base, "/api/notes")
        assert status == 200 and all(note["title"] != "After Backup" for note in json.loads(body)["notes"])
        status, _, body = request(base, "/api/admin/backup-status")
        backup_status = json.loads(body)
        assert status == 200 and backup_status["count"] == 1
        assert backup_status["last_backup"]["created_at"]
        assert backup_status["last_backup"]["created_at"].endswith("Z")
        status, _, body = request(base, "/admin/backup/list")
        backup_list = json.loads(body)
        assert status == 200 and backup_list["count"] == 1
        assert backup_list["items"][0]["created"] and backup_list["items"][0]["created_es"]

        status, _, body = request(base, note_path, "DELETE")
        assert status == 200 and json.loads(body)["deleted"] is True
        print("HTTP smoke: pages/assets, notes/search/history, calendar, AI demo, MCP auth/tools, backup/restore PASS")
    finally:
        server.stop()


class MockMcpHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    calls = []
    session_headers = []

    def log_message(self, *_args):
        pass

    def do_POST(self):
        size = int(self.headers.get("Content-Length", "0"))
        request = json.loads(self.rfile.read(size))
        method = request.get("method")
        params = request.get("params", {})
        self.__class__.calls.append((method, params))
        self.__class__.session_headers.append(self.headers.get("mcp-session-id"))

        if method == "initialize":
            payload = {"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "test-mcp", "version": "1"}}}
            self.send_response(200)
            self.send_header("mcp-session-id", "smoke-session")
            self.send_header("Content-Type", "application/json")
            encoded = json.dumps(payload).encode()
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
            return
        if method == "notifications/initialized":
            self.send_response(202)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if method == "tools/call":
            tool = params.get("name")
            args = params.get("arguments", {})
            if tool == "get_horario":
                data = {"horario": [{"dia": "Lunes", "hora": "08:00", "clase": "Matemáticas reales", "codigo": "MAT"}]}
            elif tool == "get_calendario":
                data = {"eventos": [{"fecha": "2026-11-14", "titulo": "Examen real", "tipo_dia": "EXAMEN"}]}
            elif tool in ("get_perfil", "get_student_full"):
                data = {"estudiante": {"nombre": "Ada Real"}, "perfil": {"nombre": "Ada Real"}, "calendario": [{"titulo": "Examen real"}], "horario": [{"dia": "Lunes", "clase": "Matemáticas reales"}]}
            elif tool == "get_detalle_materia":
                data = {"materia": args.get("materia", ""), "detalle": "Dato real"}
            elif tool == "get_tareas":
                data = {"lista_tareas": [{"titulo": "Tarea real"}]}
            elif tool == "get_asistencia":
                data = {"asistencia": [{"fecha": "2026-11-13", "estado": "presente"}]}
            elif tool == "create_docx":
                Path(args["out"]).write_bytes(b"PK\x03\x04mock-docx-content")
                data = {"ok": True, "path": args["out"]}
            else:
                data = {"tool": tool, "available": False}
            payload = {"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": json.dumps(data, ensure_ascii=False)}]}}
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            encoded = json.dumps(payload, ensure_ascii=False).encode()
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
            return
        self.send_response(400)
        self.send_header("Content-Length", "0")
        self.end_headers()



def run_office_parity_smoke(binary: Path, workspace: Path, temp: Path):
    """Exercise Office regressions beyond the small public-page smoke path."""
    server = Server(binary, workspace, temp / "office-parity")
    try:
        server.wait_ready()
        base = server.base
        office_form = {"Content-Type": "application/x-www-form-urlencoded", "Accept": "text/html"}

        status, headers, _ = request(base, "/office/docs", "POST", {"title": "Content Only", "content": "v1"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/docs/content-only?saved=1"
        status, headers, _ = request(base, "/office/docs/content-only", "PUT", {"content": "v2 updated"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/docs/content-only?saved=1"
        status, _, body = request(base, "/office/docs/content-only", headers={"Accept": "text/html"})
        assert status == 200 and b"v2 updated" in body
        status, _, _ = request(base, "/office/docs/%2E%2E", "PUT", {"content": "must not escape"}, office_form)
        assert status in (400, 404)
        status, _, _ = request(base, "/office/docs/content-only/preview", "POST", {}, office_form)
        assert status == 405

        status, headers, _ = request(base, "/office/sheets", "POST", {"title": "Unsupported Formula"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/sheets/unsupported-formula"
        status, headers, _ = request(base, "/office/sheets/unsupported-formula/import", "POST", {"csv_data": "5,=PRODUCT(A1:A2)\n"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/sheets/unsupported-formula"
        status, _, _ = request(base, "/office/sheets/unsupported-formula/export", "POST", {}, office_form)
        assert status == 405
        status, _, body = request(base, "/office/sheets/unsupported-formula/export")
        assert status == 200 and b"#ERROR" in body and b"unsupported formula" in body and b",=PRODUCT" not in body

        status, headers, _ = request(base, "/office/presentations", "POST", {"title": "Deck"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/deck"
        status, headers, _ = request(base, "/office/presentations/deck/slides", "POST", {"title": "Scene", "content": "old"}, office_form)
        assert status == 200
        status, _, body = request(base, "/office/presentations/deck", headers={"Accept": "text/html"})
        assert status == 200 and b"hx-put=\"/office/presentations/deck/slides/0\"" in body and b"Guardar escena" in body
        status, headers, _ = request(base, "/office/presentations/deck/slides/0", "PUT", {"title": "Scene updated", "content": "# New content"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/deck"
        status, _, body = request(base, "/office/presentations/deck/present", headers={"Accept": "text/html"})
        assert status == 200 and b"<h1>New content</h1>" in body
        status, _, _ = request(base, "/office/presentations/deck/slides", "POST", {"title": "  ", "content": "invalid"}, office_form)
        assert status == 400

        status, headers, _ = request(base, "/office/presentations", "POST", {"title": "Reorder Deck"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/reorder-deck"
        status, _, _ = request(base, "/office/presentations/reorder-deck/slides", "GET")
        assert status == 405
        status, _, _ = request(base, "/office/presentations/reorder-deck/slides/reorder", "GET")
        assert status == 405
        for title in ("Slide A", "Slide B"):
            status, headers, _ = request(base, "/office/presentations/reorder-deck/slides", "POST", {"title": title, "content": title}, office_form)
            assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/reorder-deck"
        for order in ("0,0", "0"):
            status, _, _ = request(base, "/office/presentations/reorder-deck/slides/reorder", "POST", {"order": order}, office_form)
            assert status == 400
        status, headers, _ = request(base, "/office/presentations/reorder-deck/slides/reorder", "POST", {"order": "1,0"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations/reorder-deck"

        status, headers, _ = request(base, "/office/library/import/docs", "POST", {"name": "Imported Note", "content": "# Imported\nLocal file"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/library"
        status, headers, body = request(base, "/office/library/docs/imported-note/download")
        assert status == 200 and headers.get_content_type() == "text/markdown" and b"Imported" in body
        status, headers, _ = request(base, "/office/library/import/presentations", "POST", {"name": "Imported Deck", "content": "# First scene\n\nImported content."}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/library"
        status, _, body = request(base, "/office/presentations/imported-deck", headers={"Accept": "text/html"})
        assert status == 200 and b"Imported content" in body
        valid_deck = json.dumps({"title": "JSON Deck", "slides": [{"title": "First", "content": "First scene"}, {"title": "Second", "content": "Second scene"}]})
        status, headers, _ = request(base, "/office/library/import/presentations", "POST", {"name": "Json Deck", "content": valid_deck}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/library"
        status, _, body = request(base, "/office/presentations/json-deck/present", headers={"Accept": "text/html"})
        assert status == 200 and b"Second scene" in body
        broken_deck = json.dumps({"title": "Broken", "slides": [{"title": "", "content": "invalid"}]})
        status, _, _ = request(base, "/office/library/import/presentations", "POST", {"name": "Broken", "content": broken_deck}, office_form)
        assert status == 400
        status, headers, _ = request(base, "/office/library/docs/imported-note/delete", "POST", {}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/library"
        status, _, _ = request(base, "/office/library/docs/imported-note/download")
        assert status == 404
        status, headers, _ = request(base, "/office/sheets/unsupported-formula/delete", "POST", {}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/sheets"
        status, headers, _ = request(base, "/office/presentations/reorder-deck", "DELETE", {}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/presentations"
        print("HTTP smoke: Office CRUD, validation, formula errors, reorder, imports and traversal guards PASS")
    finally:
        server.stop()


def run_mainline_merge_smoke(binary: Path, workspace: Path, temp: Path):
    """Validate newer mainline static assets, local Office formats, and calendar imports."""
    server = Server(binary, workspace, temp / "mainline-merge")
    try:
        server.wait_ready()
        base = server.base
        office_form = {"Content-Type": "application/x-www-form-urlencoded", "Accept": "text/html"}

        for path, marker in (
            ("/vendor/htmx-2.0.3.min.js", b"htmx"),
            ("/vendor/xlsx-0.20.3.full.min.js", b"XLSX"),
            ("/calendar-ics.js", b"parseIcs"),
            ("/office-sheets.js", b"workbookFromAoa"),
            ("/office-html.js", b"previewDocument"),
        ):
            status, headers, body = request(base, path)
            assert status == 200 and headers.get_content_type() == "application/javascript" and marker in body
        status, _, dashboard = request(base, "/dashboard", headers={"Accept": "text/html"})
        assert status == 200 and b"/calendar-ics.js" in dashboard

        event = {
            "date": "2026-11-20", "title": "Retiro de estudio", "body": "Material local",
            "category": "personal", "kind": "event", "end_date": "2026-11-23",
            "all_day": True, "location": "Quito", "uid": "retiro@example.test",
        }
        status, _, body = request(base, "/api/calendar/annotations", "POST", event)
        created = json_body((status, None, body))
        assert status == 201 and created["annotation"]["uid"] == "retiro@example.test"
        assert created["annotation"]["category"] == "personal" and created["end_date"] == "2026-11-23"
        status, _, body = request(base, "/api/calendar/annotations?from=2026-11-22&to=2026-11-22")
        listed = json_body((status, None, body))
        assert status == 200 and listed["count"] == 1 and listed["annotations"][0]["title"] == "Retiro de estudio"
        status, _, body = request(base, "/dashboard/calendar.ics")
        calendar = body.decode("utf-8")
        assert status == 200 and "UID:retiro@example.test" in calendar
        assert "DTEND;VALUE=DATE:20261123" in calendar and "LOCATION:Quito" in calendar

        imported_event = {
            "date": "2026-11-24", "title": "Tutoría semanal", "category": "academic",
            "kind": "event", "all_day": False, "start_time": "09:00", "end_time": "10:00",
            "end_date": "2026-11-24", "recurrence": "weekly", "weekly_day": 2,
            "uid": "tutoria@example.test",
        }
        status, _, body = request(base, "/api/calendar/annotations/import", "POST", {"annotations": [imported_event]})
        assert status == 200 and json_body((status, None, body))["imported"] == 1
        status, _, body = request(base, "/dashboard/calendar.ics")
        calendar = body.decode("utf-8")
        assert status == 200 and "DTSTART:20261124T090000" in calendar
        assert "RRULE:FREQ=WEEKLY;BYDAY=TU" in calendar
        invalid_batch = {"annotations": [
            {"date": "2026-11-25", "title": "No persistir", "kind": "event", "all_day": True},
            {"date": "2026-11-26", "title": "Inválido", "kind": "event", "category": "unknown", "all_day": True},
        ]}
        status, _, _ = request(base, "/api/calendar/annotations/import", "POST", invalid_batch)
        assert status == 400
        status, _, body = request(base, "/api/calendar/annotations?from=2026-11-25&to=2026-11-26")
        assert all(item["title"] != "No persistir" for item in json_body((status, None, body))["annotations"])

        status, headers, _ = request(base, "/office/sheets", "POST", {"title": "Excel Bridge"}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/sheets/excel-bridge"
        status, _, body = request(base, "/office/sheets/excel-bridge", headers={"Accept": "text/html"})
        assert status == 200 and b"office-xlsx-file" in body and b"office-xlsx-sheet" in body
        assert b"office-xlsx-import" in body and b"office-xlsx-export" in body
        assert b"/vendor/xlsx-0.20.3.full.min.js" in body and b"/office-sheets.js" in body

        html_source = "<!doctype html><html lang=\"es\"><body><h1>Preview</h1><script>bad()</script></body></html>"
        status, headers, _ = request(base, "/office/html", "POST", {"title": "Safe Page", "content": html_source}, office_form)
        assert status == 200 and headers.get("HX-Redirect") == "/office/html/safe-page"
        status, _, body = request(base, "/office/html/safe-page", headers={"Accept": "text/html"})
        assert status == 200 and b"sandbox=\"\"" in body and b"office-html-preview-frame" in body
        assert b"/office-html.js" in body and b"office-html-save-form" in body
        status, headers, body = request(base, "/office/html/safe-page/export")
        assert status == 200 and headers.get_content_type() == "text/html" and b"<h1>Preview</h1>" in body
        status, _, _ = request(base, "/office/html/safe-page", "PUT", {"content": "<main>updated</main>"}, office_form)
        assert status == 200
        status, _, body = request(base, "/office/library/html/safe-page/download")
        assert status == 200 and b"<main>updated</main>" in body
        status, headers, _ = request(base, "/office/html/safe-page", "DELETE")
        assert status == 200 and headers.get("HX-Redirect") == "/office/html"

        print("HTTP smoke: mainline assets, ICS event import/export, XLSX bridge, and sandboxed HTML editor PASS")
    finally:
        server.stop()

def run_external_integrations_smoke(binary: Path, workspace: Path, temp: Path):
    MockMcpHandler.calls = []
    MockMcpHandler.session_headers = []
    mock = http.server.ThreadingHTTPServer(("127.0.0.1", 0), MockMcpHandler)
    mock_thread = threading.Thread(target=mock.serve_forever, daemon=True)
    mock_thread.start()
    mock_url = f"http://127.0.0.1:{mock.server_port}/mcp"
    server = Server(binary, workspace, temp / "integrations", {
        "MIKARENO_MCP_URL": mock_url,
        "GENOFFICE_MCP_URL": mock_url,
        "MCP_TIMEOUT_MS": "1500",
    })
    try:
        server.wait_ready()
        status, _, body = request(server.base, "/dashboard/schedule")
        schedule = json.loads(body)
        assert status == 200 and schedule["connected"] is True
        assert schedule["schedule"]["horario"][0]["clase"] == "Matemáticas reales"
        status, _, body = request(server.base, "/dashboard/horario", headers={"Accept": "text/html"})
        assert status == 200 and "Matemáticas reales" in body.decode()

        headers = {"X-API-Key": "smoke-mcp-key"}
        status, _, body = request(server.base, "/mcp", "POST", {
            "jsonrpc": "2.0", "id": 12, "method": "tools/call",
            "params": {"name": "get_academic_context", "arguments": {}},
        }, headers)
        context = json.loads(body)["result"]
        assert status == 200 and context["connected"] is True and context["data"]["estudiante"]["nombre"] == "Ada Real"

        status, headers, body = request(server.base, "/office/report")
        assert status == 200 and headers.get_content_type() == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        assert body.startswith(b"PK\x03\x04")
        report_call = next(params for method, params in MockMcpHandler.calls if method == "tools/call" and params.get("name") == "create_docx")
        report_markdown = report_call["arguments"]["markdown"]
        assert "| Estudiante | Ada Real |" in report_markdown
        assert "_La fuente no devolvió promedios._" in report_markdown
        assert "| Tarea real |  | SIN_ESTADO |" in report_markdown
        assert "smoke-session" in MockMcpHandler.session_headers
        print("HTTP smoke: real-shaped Mikareno and GenOffice MCP client flows PASS")
    finally:
        server.stop()
        mock.shutdown()
        mock.server_close()
        mock_thread.join(timeout=2)


def run_auth_smoke(binary: Path, workspace: Path, temp: Path):
    server = Server(
        binary,
        workspace,
        temp / "auth",
        {
            "AUTH_DISABLED": "false",
            "REGISTRATION_TOKEN": "smoke-invite-token",
            "CUSTOM_MCP_API_KEY": "",
        },
    )
    try:
        server.wait_ready()
        status, _, _ = request(server.base, "/api/notes")
        assert status == 401
        status, headers, _ = request(server.base, "/dashboard", headers={"Accept": "text/html"})
        assert status == 302 and headers.get("Location") == "/login"
        status, headers, _ = request(server.base, "/office", headers={"Accept": "text/html"})
        assert status == 302 and headers.get("Location") == "/login"
        status, _, _ = request(server.base, "/register", "POST", {
            "username": "zig-smoke-user",
            "password": "correct horse battery staple",
            "confirm": "correct horse battery staple",
            "invite": "smoke-invite-token",
        }, {"Content-Type": "application/x-www-form-urlencoded"})
        assert status == 200
        status, headers, _ = request(server.base, "/login", "POST", {
            "username": "zig-smoke-user",
            "password": "correct horse battery staple",
        }, {"Content-Type": "application/x-www-form-urlencoded"})
        assert status == 302 and headers.get("Location") == "/dashboard"
        cookie = headers.get("Set-Cookie")
        assert cookie and "HttpOnly" in cookie and "SameSite=Strict" in cookie
        cookie_header = {"Cookie": cookie.split(";", 1)[0]}
        status, _, _ = request(server.base, "/api/notes", headers=cookie_header)
        assert status == 200
        status, _, _ = request(server.base, "/mcp", "POST", {"jsonrpc": "2.0", "id": 4, "method": "tools/list"})
        assert status == 401
        status, headers, _ = request(server.base, "/logout", "POST", headers=cookie_header)
        assert status == 200 and headers.get("Set-Cookie", "").endswith("Max-Age=0")
        status, _, _ = request(server.base, "/api/notes")
        assert status == 401
        print("HTTP smoke: registration, login/session cookies, protected routes, keyless MCP disabled PASS")
    finally:
        server.stop()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="zig-out/bin/study-platform")
    parser.add_argument("--workspace", default=str(Path(__file__).resolve().parents[1]))
    args = parser.parse_args()
    workspace = Path(args.workspace).resolve()
    binary = Path(args.binary)
    if not binary.is_absolute():
        binary = (workspace / binary).resolve()
    if not binary.is_file():
        raise SystemExit(f"Zig server binary not found: {binary}")

    version = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True)
    assert "Zig" in version.stdout + version.stderr
    with tempfile.TemporaryDirectory(prefix="study-platform-zig-smoke-") as temporary:
        temp = Path(temporary)
        run_public_smoke(binary, workspace, temp)
        run_office_parity_smoke(binary, workspace, temp)
        run_mainline_merge_smoke(binary, workspace, temp)
        run_external_integrations_smoke(binary, workspace, temp)
        run_auth_smoke(binary, workspace, temp)
    print("Zig HTTP smoke suite: PASS")


if __name__ == "__main__":
    main()
