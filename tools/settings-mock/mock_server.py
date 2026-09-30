#!/usr/bin/env python3
"""設定ページ (web/settings/index.html) をホストで確かめるための偽の端末 (0.5.0〜、docs/settings-server.md)

ファームウェアが送るのと同じ HTML を gzip にして返し、API (/api/status, /api/settings, /api/images, /img/NAME,
POST の各 API) を端末と同じ形・同じ検査 (アクセスコード、Origin、BMP のヘッダ、8.3 の名前) で真似る。
写真は tools/ui-sim/samples と リポジトリ直下の IMAGE*.BMP を SD の中身として使い、追加 / 削除はメモリの中だけ。

    python3 tools/settings-mock/mock_server.py            # http://127.0.0.1:8080/  (コード 123456)
    python3 tools/settings-mock/mock_server.py --port 9000 --code 000000 --slow 0.2

--slow で応答を遅らせると、端末の 1 本ずつの接続に近い感触になる。
"""
import argparse
import gzip
import json
import os
import re
import struct
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
PAGE = os.path.join(ROOT, "web", "settings", "index.html")
UPLOAD_SIZE = 115_254
MAX_FILES = 16
SHORT = re.compile(r"^[A-Za-z0-9_\-~!#$%&'()@^{}]{1,8}\.[A-Za-z0-9_\-~!#$%&'()@^{}]{1,3}$")

LOCK = threading.Lock()
STATE = {
    "settings": {
        "place": "東京",
        "lat": 35.6812,
        "lon": 139.7671,
        "tz_offset_secs": 32400,
        "layout": "glass",
        "slide": 30,
        "status": "auto",
        "scroll": 1,
        "message_url": "https://raw.githubusercontent.com/Droplet-Collective/aki300yenLCD-raspico2w/main/ticker/message.txt",
        "local_message": False,
        "message": "",
        "images": "",
        "sd": True,
    },
    "files": {},  # 名前 → bytes
    "boot": time.time(),
    "failures": 0,
    "locked_until": 0.0,
}


def load_samples():
    sources = [
        os.path.join(ROOT, "IMAGE.BMP"),
        os.path.join(ROOT, "IMAGE2.BMP"),
        os.path.join(ROOT, "tools", "ui-sim", "samples", "SUNSET.BMP"),
        os.path.join(ROOT, "tools", "ui-sim", "samples", "CLOUDS.BMP"),
        os.path.join(ROOT, "tools", "ui-sim", "samples", "BOKEH.BMP"),
    ]
    for path in sources:
        if os.path.exists(path):
            STATE["files"][os.path.basename(path).upper()] = open(path, "rb").read()


def check_bmp(data):
    if len(data) != UPLOAD_SIZE:
        return "size must be 115254 bytes (400x96 24-bit BMP)"
    if data[:2] != b"BM":
        return "not a BMP"
    size, = struct.unpack_from("<I", data, 2)
    off, dib, w, h, planes, bpp, comp = struct.unpack_from("<IIiiHHI", data, 10)
    if size != UPLOAD_SIZE or off != 54 or dib != 40 or w != 400 or abs(h) != 96 or planes != 1 or bpp != 24 or comp != 0:
        return "not a 400x96 24-bit BMP"
    return None


def short_name(original):
    stem = re.sub(r"\.[^.]*$", "", original).upper()
    stem = re.sub(r"[^A-Z0-9_\-]", "", stem).lstrip("-_")[:8].rstrip("-_")
    return (stem + ".BMP") if stem else None


class Handler(BaseHTTPRequestHandler):
    server_version = "ticker-mock"
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        if not self.server.quiet:
            super().log_message(fmt, *args)

    # ---- 応答 ----
    def send(self, status, body=b"", ctype="application/json; charset=utf-8", extra=()):
        time.sleep(self.server.slow)
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def json(self, obj, status=200):
        self.send(status, json.dumps(obj, ensure_ascii=False).encode())

    def fail(self, status, message, **extra):
        self.json({"ok": False, "status": status, "error": message, **extra}, status)

    # ---- GET ----
    def do_GET(self):
        path = urllib.parse.urlsplit(self.path).path
        if path in ("/", "/index.html"):
            body = gzip.compress(open(PAGE, "rb").read(), 9)
            return self.send(200, body, "text/html; charset=utf-8", [("Content-Encoding", "gzip")])
        if path == "/api/status":
            up = int(time.time() - STATE["boot"]) + 3 * 3600 + 12 * 60
            return self.json({
                "version": "0.5.0",
                "uptime_s": up,
                "ssid": "aterm-abff4a-g",
                "ip": "192.168.200.130",
                "rssi": None,
                "wifi": "aterm-abff4a-g 192.168.200.130",
                "ntp": "ok s1",
                "weather": "ok",
                "message_state": "ok(local)" if STATE["settings"]["local_message"] else "ok",
                "ota": "OTA: up to date (latest 0.5.0), next check in 41s",
                "ota_tone": "ok",
                "ident": "ticker v0.5.0 via OTA",
                "tbyb": "slot B TBYB:bought OK stk 23.0/35.5K",
                "stack_used": 23552,
                "stack_total": 36380,
                "last_reset": "power-on / reset pin",
                "layout": STATE["settings"]["layout"],
                "weather_now": {"temperature": 19.1, "code": 2, "condition": "晴れ時々くもり"},
                "last_ota_check_s": 19,
                "ota_checks": 193,
                "pending": False,
            })
        if path == "/api/settings":
            return self.json(STATE["settings"])
        if path == "/api/images":
            order = [n for n in STATE["settings"]["images"].split(",") if n]
            files = [{"name": n, "size": len(b)} for n, b in STATE["files"].items()]
            return self.json({"files": files, "order": order, "max": MAX_FILES, "upload_size": UPLOAD_SIZE})
        if path.startswith("/img/"):
            name = urllib.parse.unquote(path[5:]).upper()
            data = STATE["files"].get(name)
            if data is None:
                return self.fail(404, "写真がありません")
            return self.send(200, data, "image/bmp")
        return self.fail(404, "ありません")

    def do_OPTIONS(self):
        self.send(405, b"", extra=[("Allow", "GET, POST")])

    # ---- POST ----
    def do_POST(self):
        url = urllib.parse.urlsplit(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length <= 200_000 else b""
        origin = self.headers.get("Origin")
        host = self.headers.get("Host")
        if origin and origin != f"http://{host}":
            return self.fail(403, "よそのページからの要求は受け付けません")
        if url.path == "/api/show-code":
            print(f"[mock] LCD: settings: http://{host}/  code {self.server.code}", flush=True)
            return self.json({"ok": True})
        with LOCK:
            if time.time() < STATE["locked_until"]:
                return self.fail(429, "コードを続けて間違えたので、しばらく受け付けません",
                                 retry_after=int(STATE["locked_until"] - time.time()) + 1)
            given = (self.headers.get("X-Ticker-Code") or "").strip()
            if not given:
                return self.fail(401, "アクセスコードが要ります")
            if given != self.server.code:
                STATE["failures"] += 1
                if STATE["failures"] >= 5:
                    STATE["failures"] = 0
                    STATE["locked_until"] = time.time() + 30
                    return self.fail(429, "コードを続けて間違えたので、しばらく受け付けません", retry_after=30)
                return self.fail(401, "アクセスコードが違います")
            STATE["failures"] = 0
        if url.path == "/api/auth":
            return self.json({"ok": True})
        if url.path == "/api/settings":
            return self.save(urllib.parse.parse_qs(body.decode(), keep_blank_values=True))
        if url.path == "/api/images/delete":
            name = urllib.parse.parse_qs(body.decode()).get("name", [""])[0].upper()
            if name not in STATE["files"]:
                return self.fail(404, "消せませんでした (ありません)")
            del STATE["files"][name]
            order = [n for n in STATE["settings"]["images"].split(",") if n and n.upper() != name]
            STATE["settings"]["images"] = ",".join(order)
            return self.json({"ok": True})
        if url.path == "/api/upload":
            if length != UPLOAD_SIZE:
                return self.fail(413, "400×96 の 24 bit BMP (115,254 バイト) だけ受け付けます")
            problem = check_bmp(body)
            if problem:
                return self.fail(415, "400×96 の 24 bit BMP ではありません")
            if len(STATE["files"]) >= MAX_FILES:
                return self.fail(409, "写真は 16 枚までです。先に消してください")
            original = urllib.parse.parse_qs(url.query).get("name", [""])[0]
            name = short_name(original) or "IMG00001.BMP"
            n = 2
            while name in STATE["files"]:
                name = name.split(".")[0][:6] + f"~{n}.BMP"
                n += 1
            STATE["files"][name] = body
            if STATE["settings"]["images"]:
                STATE["settings"]["images"] += "," + name
            time.sleep(1.5)  # SD に書く時間
            return self.json({"ok": True, "name": name})
        if url.path == "/api/reboot":
            return self.json({"ok": True})
        if url.path == "/api/ota-check":
            return self.json({"ok": True})
        return self.fail(404, "ありません")

    def save(self, form):
        s = STATE["settings"]
        allowed = {"place", "lat", "lon", "tz", "layout", "slide", "status", "scroll", "message", "message_url", "images"}
        for key, values in form.items():
            if key not in allowed:
                return self.fail(400, "知らない設定の名前です")
            v = values[0].strip()
            try:
                if key == "place":
                    assert v and len(v.encode()) <= 32 and "#" not in v
                    s["place"] = v
                elif key in ("lat", "lon"):
                    f = float(v)
                    assert -90 <= f <= 90 if key == "lat" else -180 <= f <= 180
                    s[key] = f
                elif key == "tz":
                    m = re.fullmatch(r"([+-]?)(\d{1,2})(?::(\d{2}))?", v)
                    secs = int(m.group(2)) * 3600 + int(m.group(3) or 0) * 60
                    s["tz_offset_secs"] = -secs if m.group(1) == "-" else secs
                elif key == "layout":
                    assert v in ("glass", "dock", "classic")
                    s["layout"] = v
                elif key == "slide":
                    n = int(v)
                    assert n == 0 or 5 <= n <= 3600
                    s["slide"] = n
                elif key == "status":
                    assert v in ("auto", "full", "compact")
                    s["status"] = v
                elif key == "scroll":
                    n = int(v)
                    assert 1 <= n <= 8
                    s["scroll"] = n
                elif key == "message":
                    assert len(v.encode()) <= 512
                    s["message"] = v
                    s["local_message"] = bool(v)
                elif key == "message_url":
                    assert v.startswith(("http://", "https://")) and "#" not in v
                    s["message_url"] = v
                elif key == "images":
                    names = [n for n in v.split(",") if n]
                    assert all(SHORT.match(n) for n in names)
                    s["images"] = v
            except (AssertionError, ValueError, AttributeError):
                return self.fail(422, "値が正しくありません")
        return self.json({"ok": True, "changed": len(form)})


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=8080)
    ap.add_argument("--bind", default="127.0.0.1")
    ap.add_argument("--code", default="123456", help="アクセスコード (端末は起動ごとに作る 6 桁)")
    ap.add_argument("--slow", type=float, default=0.0, help="応答ごとの遅れ (秒)")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()
    load_samples()
    srv = ThreadingHTTPServer((args.bind, args.port), Handler)
    srv.code, srv.slow, srv.quiet = args.code, args.slow, args.quiet
    print(f"settings mock: http://{args.bind}:{args.port}/  (code {args.code})", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
