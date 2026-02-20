"""
SFTP E2E tests via Den HTTP API.

Tests connect to a running Den server, then use its SFTP API
to operate on a real SSH/SFTP host.

Requirements:
    pip install requests

Usage:
    # 1) Start Den server:
    #   $env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; cargo run
    #
    # 2) Ensure an SSH server is reachable (e.g. localhost OpenSSH):
    #   DEN_SFTP_HOST=localhost DEN_SFTP_USER=myuser DEN_SFTP_PASS=mypass python tests/sftp_e2e_test.py

Environment variables:
    DEN_URL          Den base URL          (default: http://127.0.0.1:3000)
    DEN_PASSWORD     Den login password     (default: test)
    DEN_SFTP_HOST    SFTP target host       (required)
    DEN_SFTP_PORT    SFTP target port       (default: 22)
    DEN_SFTP_USER    SFTP username          (required)
    DEN_SFTP_PASS    SFTP password          (required)
"""

import os
import sys
import time
import unittest

import requests

DEN_URL = os.environ.get("DEN_URL", "http://127.0.0.1:3000")
DEN_PASSWORD = os.environ.get("DEN_PASSWORD", "test")
SFTP_HOST = os.environ.get("DEN_SFTP_HOST", "")
SFTP_PORT = int(os.environ.get("DEN_SFTP_PORT", "22"))
SFTP_USER = os.environ.get("DEN_SFTP_USER", "")
SFTP_PASS = os.environ.get("DEN_SFTP_PASS", "")

# Test directory on the remote host (created/cleaned by tests)
TEST_DIR = f"/tmp/den-sftp-e2e-{int(time.time())}"


class DenSession:
    """Manages a Den HTTP session with auth cookie."""

    def __init__(self):
        self.session = requests.Session()
        self._login()

    def _login(self):
        resp = self.session.post(
            f"{DEN_URL}/api/login",
            json={"password": DEN_PASSWORD},
        )
        resp.raise_for_status()

    def get(self, path, **kwargs):
        return self.session.get(f"{DEN_URL}{path}", **kwargs)

    def post(self, path, **kwargs):
        return self.session.post(f"{DEN_URL}{path}", **kwargs)

    def put(self, path, **kwargs):
        return self.session.put(f"{DEN_URL}{path}", **kwargs)

    def delete(self, path, **kwargs):
        return self.session.delete(f"{DEN_URL}{path}", **kwargs)


# Shared session for all tests
_den = None


def den():
    global _den
    if _den is None:
        _den = DenSession()
    return _den


class TestSftpConnect(unittest.TestCase):
    """Test SFTP connect/disconnect/status lifecycle."""

    def test_01_status_disconnected(self):
        resp = den().get("/api/sftp/status")
        self.assertEqual(resp.status_code, 200)
        data = resp.json()
        self.assertFalse(data["connected"])

    def test_02_connect(self):
        resp = den().post(
            "/api/sftp/connect",
            json={
                "host": SFTP_HOST,
                "port": SFTP_PORT,
                "username": SFTP_USER,
                "auth_type": "password",
                "password": SFTP_PASS,
            },
        )
        self.assertEqual(resp.status_code, 200, resp.text)
        data = resp.json()
        self.assertTrue(data["connected"])
        self.assertIn(SFTP_HOST, data["host"])
        self.assertEqual(data["username"], SFTP_USER)

    def test_03_status_connected(self):
        resp = den().get("/api/sftp/status")
        self.assertEqual(resp.status_code, 200)
        data = resp.json()
        self.assertTrue(data["connected"])

    def test_04_connect_wrong_password(self):
        resp = den().post(
            "/api/sftp/connect",
            json={
                "host": SFTP_HOST,
                "port": SFTP_PORT,
                "username": SFTP_USER,
                "auth_type": "password",
                "password": "wrong_password_xyz",
            },
        )
        # Should fail with 401 (AuthFailed) or 502 (SSH error)
        self.assertIn(resp.status_code, [401, 502], resp.text)

    def test_05_reconnect(self):
        """Reconnect after failed auth."""
        resp = den().post(
            "/api/sftp/connect",
            json={
                "host": SFTP_HOST,
                "port": SFTP_PORT,
                "username": SFTP_USER,
                "auth_type": "password",
                "password": SFTP_PASS,
            },
        )
        self.assertEqual(resp.status_code, 200, resp.text)
        self.assertTrue(resp.json()["connected"])


class TestSftpFileOps(unittest.TestCase):
    """Test file operations (requires connected SFTP)."""

    @classmethod
    def setUpClass(cls):
        # Ensure connected
        resp = den().get("/api/sftp/status")
        if not resp.json().get("connected"):
            den().post(
                "/api/sftp/connect",
                json={
                    "host": SFTP_HOST,
                    "port": SFTP_PORT,
                    "username": SFTP_USER,
                    "auth_type": "password",
                    "password": SFTP_PASS,
                },
            )
        # Create test directory
        den().post("/api/sftp/mkdir", json={"path": TEST_DIR})

    def test_01_mkdir(self):
        path = f"{TEST_DIR}/subdir"
        resp = den().post("/api/sftp/mkdir", json={"path": path})
        self.assertEqual(resp.status_code, 200, resp.text)

    def test_02_write_and_read(self):
        path = f"{TEST_DIR}/hello.txt"
        resp = den().put(
            "/api/sftp/write",
            json={"path": path, "content": "Hello, SFTP!"},
        )
        self.assertEqual(resp.status_code, 200, resp.text)

        resp = den().get("/api/sftp/read", params={"path": path})
        self.assertEqual(resp.status_code, 200, resp.text)
        data = resp.json()
        self.assertEqual(data["content"], "Hello, SFTP!")
        self.assertFalse(data["binary"])

    def test_03_list(self):
        resp = den().get(
            "/api/sftp/list",
            params={"path": TEST_DIR, "show_hidden": "false"},
        )
        self.assertEqual(resp.status_code, 200, resp.text)
        data = resp.json()
        names = [e["name"] for e in data["entries"]]
        self.assertIn("hello.txt", names)
        self.assertIn("subdir", names)

    def test_04_rename(self):
        old = f"{TEST_DIR}/hello.txt"
        new = f"{TEST_DIR}/renamed.txt"
        resp = den().post(
            "/api/sftp/rename",
            json={"from": old, "to": new},
        )
        self.assertEqual(resp.status_code, 200, resp.text)

        # Verify old is gone, new exists
        resp = den().get("/api/sftp/read", params={"path": new})
        self.assertEqual(resp.status_code, 200, resp.text)
        self.assertEqual(resp.json()["content"], "Hello, SFTP!")

    def test_05_upload_and_download(self):
        path = TEST_DIR
        resp = den().post(
            "/api/sftp/upload",
            data={"path": path},
            files={"file": ("upload_test.txt", b"uploaded content", "text/plain")},
        )
        self.assertEqual(resp.status_code, 200, resp.text)

        # Download
        resp = den().get(
            "/api/sftp/download",
            params={"path": f"{path}/upload_test.txt"},
        )
        self.assertEqual(resp.status_code, 200, resp.text)
        self.assertEqual(resp.content, b"uploaded content")

    def test_06_search(self):
        # Write a searchable file
        path = f"{TEST_DIR}/searchable.txt"
        den().put(
            "/api/sftp/write",
            json={"path": path, "content": "needle_in_haystack"},
        )

        resp = den().get(
            "/api/sftp/search",
            params={"path": TEST_DIR, "query": "searchable", "content": "false"},
        )
        self.assertEqual(resp.status_code, 200, resp.text)
        results = resp.json()["results"]
        found = any("searchable.txt" in r["path"] for r in results)
        self.assertTrue(found, f"searchable.txt not found in results: {results}")

    def test_07_delete_file(self):
        path = f"{TEST_DIR}/renamed.txt"
        resp = den().delete("/api/sftp/delete", params={"path": path})
        self.assertEqual(resp.status_code, 200, resp.text)

        # Verify deleted
        resp = den().get("/api/sftp/read", params={"path": path})
        self.assertNotEqual(resp.status_code, 200)

    def test_08_delete_directory(self):
        path = f"{TEST_DIR}/subdir"
        resp = den().delete("/api/sftp/delete", params={"path": path})
        self.assertEqual(resp.status_code, 200, resp.text)


class TestSftpDisconnect(unittest.TestCase):
    """Test disconnect (runs last)."""

    def test_01_disconnect(self):
        resp = den().post("/api/sftp/disconnect")
        self.assertEqual(resp.status_code, 200)

    def test_02_ops_after_disconnect_return_503(self):
        resp = den().get("/api/sftp/list", params={"path": "/", "show_hidden": "false"})
        self.assertEqual(resp.status_code, 503)

    def test_03_status_after_disconnect(self):
        resp = den().get("/api/sftp/status")
        data = resp.json()
        self.assertFalse(data["connected"])


if __name__ == "__main__":
    # Validate required env vars
    missing = []
    if not SFTP_HOST:
        missing.append("DEN_SFTP_HOST")
    if not SFTP_USER:
        missing.append("DEN_SFTP_USER")
    if not SFTP_PASS:
        missing.append("DEN_SFTP_PASS")

    if missing:
        print(f"Missing required environment variables: {', '.join(missing)}")
        print()
        print("Usage:")
        print("  DEN_SFTP_HOST=host DEN_SFTP_USER=user DEN_SFTP_PASS=pass python tests/sftp_e2e_test.py")
        sys.exit(1)

    # Check Den connectivity
    print(f"Testing Den at {DEN_URL}")
    try:
        resp = requests.get(f"{DEN_URL}/api/sftp/status", timeout=5)
        # Will get 401 without auth, but proves server is up
        print(f"Den reachable (status {resp.status_code})")
    except Exception as e:
        print(f"Cannot connect to Den: {e}")
        print('Start Den: $env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; cargo run')
        sys.exit(1)

    print(f"SFTP target: {SFTP_USER}@{SFTP_HOST}:{SFTP_PORT}")
    print(f"Test dir: {TEST_DIR}")
    print()

    unittest.main(verbosity=2)
