"""Native automation helpers shared by current desktop browser tests."""
import json
import os
import subprocess
from pathlib import Path
from urllib.request import Request, urlopen

class NativeAutomation:
    @classmethod
    def ui(cls, path: str, body: dict | None = None) -> bytes:
        request = Request(
            cls.ui_url + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={
                "Authorization": "Bearer zork-shell-fixture",
                "Content-Type": "application/json",
            },
        )
        with urlopen(request, timeout=5) as response:
            return response.read()


    @classmethod
    def element(cls, element_id: str, *, enabled: bool = False) -> dict | None:
        snapshot = json.loads(cls.ui("/v1/elements"))
        return next(
            (e for e in snapshot["elements"]
             if e["id"] == element_id and e["visible"] and (not enabled or e["enabled"])),
            None,
        )


    @classmethod
    def screenshot(cls, name: str) -> None:
        if directory := os.environ.get("ZORK_GUI_SCREENSHOT_DIR"):
            path = Path(directory)
            path.mkdir(parents=True, exist_ok=True)
            (path / name).write_bytes(cls.ui("/v1/screenshot"))


    @staticmethod
    def stop_process(process: subprocess.Popen[bytes]) -> None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


    @classmethod
    def messages(cls, session_id: str) -> list[dict[str, object]]:
        status, body = cls.request(
            "GET", f"/v1/im/sessions/{session_id}/messages?limit=100"
        )
        if status != 200:
            raise AssertionError((status, body))
        return list(body["items"])
