"""
SSH integration tests for Den's built-in SSH server.

Requirements:
    pip install paramiko

Usage:
    # Start server first:
    #   $env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; $env:DEN_SSH_PORT="2222"; cargo run
    #
    # Then run tests:
    python tests/ssh_test.py
    #
    # Custom host/port:
    DEN_TEST_SSH_HOST=192.168.1.10 DEN_TEST_SSH_PORT=2222 python tests/ssh_test.py
"""

import os
import sys
import time
import unittest

import paramiko

SSH_HOST = os.environ.get("DEN_TEST_SSH_HOST", "127.0.0.1")
SSH_PORT = int(os.environ.get("DEN_TEST_SSH_PORT", "2222"))
SSH_USER = "den"
SSH_PASS = os.environ.get("DEN_TEST_SSH_PASS", "test")

# russh の auth_rejection_time (3s) より長く設定
AUTH_TIMEOUT = 15


def ssh_connect():
    """Create and return a connected SSH client."""
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        SSH_HOST,
        port=SSH_PORT,
        username=SSH_USER,
        password=SSH_PASS,
        timeout=10,
        auth_timeout=AUTH_TIMEOUT,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def exec_simple(client, command):
    """Execute a non-interactive SSH command and return stdout."""
    channel = client.get_transport().open_session()
    channel.exec_command(command)
    channel.settimeout(5.0)
    output = b""
    try:
        while True:
            data = channel.recv(4096)
            if not data:
                break
            output += data
    except Exception:
        pass
    channel.close()
    return output.decode("utf-8", errors="replace")


def exec_pty(client, command, width=80, height=24, duration=6):
    """Execute a PTY-attached SSH command, respond to DSR queries, return output."""
    channel = client.get_transport().open_session()
    channel.get_pty(term="xterm-256color", width=width, height=height)
    channel.exec_command(command)
    channel.settimeout(1.0)

    all_output = b""
    cpr_sent = False
    start = time.time()

    while time.time() - start < duration:
        try:
            data = channel.recv(4096)
            if not data:
                break
            all_output += data

            # Respond to DSR query: ESC[6n -> ESC[1;1R
            if b"\x1b[6n" in data and not cpr_sent:
                channel.send(b"\x1b[1;1R")
                cpr_sent = True
        except Exception:
            pass

    return channel, all_output


class TestSSHList(unittest.TestCase):
    """Test the 'list' command (non-interactive)."""

    def test_list_returns_output(self):
        client = ssh_connect()
        output = exec_simple(client, "list")
        # Should return either "No active sessions" or "Sessions:"
        self.assertTrue(
            "No active sessions" in output or "Sessions:" in output,
            f"Unexpected list output: {output!r}",
        )
        client.close()


class TestSSHNewSession(unittest.TestCase):
    """Test creating a new PTY session via SSH."""

    def setUp(self):
        self.client = ssh_connect()
        self.session_name = f"ssh-test-{int(time.time())}"
        self.channel = None

    def tearDown(self):
        if self.channel and not self.channel.closed:
            self.channel.close()
        self.client.close()

    def test_new_session_shows_prompt(self):
        """Creating a new session should show shell prompt."""
        self.channel, output = exec_pty(self.client, f"new {self.session_name}")
        text = output.decode("utf-8", errors="replace")

        # Shell prompt should contain recognizable characters
        has_prompt = ">" in text or "PS " in text or "$" in text
        self.assertTrue(has_prompt, f"Shell prompt not found in output: {text!r}")

    def test_new_session_input_works(self):
        """Typing into the session should produce output."""
        self.channel, output = exec_pty(
            self.client, f"new {self.session_name}", duration=4
        )
        text = output.decode("utf-8", errors="replace")

        if ">" not in text and "PS " not in text and "$" not in text:
            self.skipTest("Shell prompt did not appear")

        # Send a command
        self.channel.send(b"echo HELLO_SSH_TEST\r\n")
        time.sleep(2)

        try:
            extra = self.channel.recv(4096)
            extra_text = extra.decode("utf-8", errors="replace")
            self.assertIn(
                "HELLO_SSH_TEST", extra_text, "echo output not received"
            )
        except Exception as e:
            self.fail(f"Failed to receive echo output: {e}")

    def test_da_response_filtered(self):
        """DA responses should be filtered and not appear as shell input."""
        self.channel, output = exec_pty(
            self.client, f"new {self.session_name}", duration=4
        )
        text = output.decode("utf-8", errors="replace")

        if ">" not in text and "PS " not in text and "$" not in text:
            self.skipTest("Shell prompt did not appear")

        # Send a DA response (should be filtered by the server)
        self.channel.send(b"\x1b[?1;2c")
        time.sleep(1)

        # Send a known command to check the shell is still clean
        self.channel.send(b"echo DA_FILTER_OK\r\n")
        time.sleep(2)

        try:
            extra = self.channel.recv(8192)
            extra_text = extra.decode("utf-8", errors="replace")
            # The DA response should NOT appear as garbled text before our echo
            self.assertIn("DA_FILTER_OK", extra_text)
            # Check that the raw DA sequence didn't leak into shell output
            self.assertNotIn("[?1;2c", extra_text)
        except Exception as e:
            self.fail(f"Failed to verify DA filter: {e}")


class TestSSHAttach(unittest.TestCase):
    """Test attaching to an existing session."""

    def setUp(self):
        self.clients = []
        self.channels = []
        self.session_name = f"ssh-attach-{int(time.time())}"

    def tearDown(self):
        for ch in self.channels:
            if not ch.closed:
                ch.close()
        for c in self.clients:
            c.close()

    def test_attach_existing_session(self):
        """Attaching to an existing session should show replay data."""
        # Create session with first client
        client1 = ssh_connect()
        self.clients.append(client1)
        ch1, output1 = exec_pty(client1, f"new {self.session_name}", duration=4)
        self.channels.append(ch1)

        text1 = output1.decode("utf-8", errors="replace")
        if ">" not in text1 and "PS " not in text1 and "$" not in text1:
            self.skipTest("Shell prompt did not appear")

        # Attach with second client
        client2 = ssh_connect()
        self.clients.append(client2)
        ch2, output2 = exec_pty(client2, f"attach {self.session_name}", duration=4)
        self.channels.append(ch2)

        text2 = output2.decode("utf-8", errors="replace")
        # Replay should contain part of the prompt
        has_prompt = ">" in text2 or "PS " in text2 or "$" in text2
        self.assertTrue(has_prompt, f"Replay data should contain prompt: {text2!r}")


class TestSSHDsrDelivery(unittest.TestCase):
    """Test that ConPTY's DSR query reaches the client via broadcast.

    This verifies the fix for the race condition where the broadcast
    subscriber was created after the read_task started, causing initial
    PTY output (including DSR) to be lost.
    """

    def setUp(self):
        self.client = ssh_connect()
        self.session_name = f"ssh-dsr-{int(time.time())}"
        self.channel = None

    def tearDown(self):
        if self.channel and not self.channel.closed:
            self.channel.close()
        self.client.close()

    def test_dsr_arrives_without_manual_cpr(self):
        """DSR query (ESC[6n) should arrive at client via broadcast."""
        channel = self.client.get_transport().open_session()
        channel.get_pty(term="xterm-256color", width=80, height=24)
        channel.exec_command(f"new {self.session_name}")
        channel.settimeout(1.0)
        self.channel = channel

        # CPR を送らずにデータを受信し、DSR が届くか確認
        data = b""
        start = time.time()
        while time.time() - start < 5:
            try:
                chunk = channel.recv(4096)
                if not chunk:
                    break
                data += chunk
                # DSR を検出したら即終了（成功）
                if b"\x1b[6n" in data:
                    break
            except Exception:
                pass

        self.assertIn(
            b"\x1b[6n",
            data,
            "DSR query not received — broadcast subscriber race condition?",
        )


class TestSSHSessionList(unittest.TestCase):
    """Test that created sessions appear in list."""

    def test_new_session_appears_in_list(self):
        client = ssh_connect()
        session_name = f"ssh-list-{int(time.time())}"

        # Create session (non-interactive, will disconnect but session persists)
        channel = client.get_transport().open_session()
        channel.get_pty(term="xterm-256color", width=80, height=24)
        channel.exec_command(f"new {session_name}")
        time.sleep(2)
        channel.close()
        client.close()

        # Check list
        client2 = ssh_connect()
        output = exec_simple(client2, "list")
        self.assertIn(session_name, output, f"Session {session_name} not in list")
        self.assertIn("alive", output)
        client2.close()


class TestSSHAuthRejection(unittest.TestCase):
    """Test that wrong password is rejected."""

    def test_wrong_password_rejected(self):
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        with self.assertRaises(paramiko.AuthenticationException):
            client.connect(
                SSH_HOST,
                port=SSH_PORT,
                username=SSH_USER,
                password="wrong_password_12345",
                timeout=10,
                auth_timeout=AUTH_TIMEOUT,
                allow_agent=False,
                look_for_keys=False,
            )


class TestSSHInvalidSession(unittest.TestCase):
    """Test that invalid session names are handled properly."""

    def test_invalid_session_name(self):
        client = ssh_connect()
        channel = client.get_transport().open_session()
        channel.get_pty(term="xterm-256color", width=80, height=24)
        channel.exec_command("new ../bad")
        channel.settimeout(5.0)

        output = b""
        try:
            while True:
                data = channel.recv(4096)
                if not data:
                    break
                output += data
        except Exception:
            pass

        text = output.decode("utf-8", errors="replace")
        # Server should reject invalid name or report error
        has_error = (
            "Invalid" in text
            or "already exists" in text.lower()
            or channel.exit_status_ready()
        )
        self.assertTrue(
            has_error or channel.closed,
            f"Expected error or channel close for invalid name, got: {text!r}",
        )
        channel.close()
        client.close()


class TestSSHWindowResize(unittest.TestCase):
    """Test that window resize works during a session."""

    def setUp(self):
        self.client = ssh_connect()
        self.session_name = f"ssh-resize-{int(time.time())}"
        self.channel = None

    def tearDown(self):
        if self.channel and not self.channel.closed:
            self.channel.close()
        self.client.close()

    def test_resize_then_echo(self):
        """After resize, echo should still work."""
        self.channel, output = exec_pty(
            self.client, f"new {self.session_name}", duration=4
        )
        text = output.decode("utf-8", errors="replace")

        if ">" not in text and "PS " not in text and "$" not in text:
            self.skipTest("Shell prompt did not appear")

        # Resize
        self.channel.resize_pty(width=120, height=40)
        time.sleep(1)

        # Send a command after resize
        self.channel.send(b"echo RESIZE_OK\r\n")
        time.sleep(2)

        try:
            extra = self.channel.recv(8192)
            extra_text = extra.decode("utf-8", errors="replace")
            self.assertIn("RESIZE_OK", extra_text)
        except Exception as e:
            self.fail(f"Failed to receive echo after resize: {e}")


class TestSSHMultipleClients(unittest.TestCase):
    """Test that multiple clients can see the same session output."""

    def setUp(self):
        self.clients = []
        self.channels = []
        self.session_name = f"ssh-multi-{int(time.time())}"

    def tearDown(self):
        for ch in self.channels:
            if not ch.closed:
                ch.close()
        for c in self.clients:
            c.close()

    def test_two_clients_receive_output(self):
        """Both clients attached to the same session should receive echo output."""
        # Client 1: create session
        client1 = ssh_connect()
        self.clients.append(client1)
        ch1, output1 = exec_pty(client1, f"new {self.session_name}", duration=4)
        self.channels.append(ch1)

        text1 = output1.decode("utf-8", errors="replace")
        if ">" not in text1 and "PS " not in text1 and "$" not in text1:
            self.skipTest("Shell prompt did not appear")

        # Client 2: attach to same session
        client2 = ssh_connect()
        self.clients.append(client2)
        ch2, output2 = exec_pty(
            client2, f"attach {self.session_name}", duration=4
        )
        self.channels.append(ch2)

        # Send echo from client 1
        ch1.send(b"echo MULTI_CLIENT_TEST\r\n")
        time.sleep(3)

        # Both clients should receive the output
        try:
            data2 = ch2.recv(8192)
            text2 = data2.decode("utf-8", errors="replace")
            self.assertIn(
                "MULTI_CLIENT_TEST",
                text2,
                "Client 2 did not receive echo output",
            )
        except Exception as e:
            self.fail(f"Client 2 failed to receive output: {e}")


class TestSSHReconnect(unittest.TestCase):
    """Test disconnect and reconnect with replay."""

    def setUp(self):
        self.session_name = f"ssh-reconn-{int(time.time())}"

    def test_reconnect_has_replay(self):
        """After disconnect+reconnect, replay should contain the marker."""
        # Session 1: create and send marker
        client1 = ssh_connect()
        ch1, output1 = exec_pty(client1, f"new {self.session_name}", duration=4)
        text1 = output1.decode("utf-8", errors="replace")

        if ">" not in text1 and "PS " not in text1 and "$" not in text1:
            ch1.close()
            client1.close()
            self.skipTest("Shell prompt did not appear")

        ch1.send(b"echo RECONNECT_MARKER_42\r\n")
        time.sleep(2)

        # Disconnect
        ch1.close()
        client1.close()
        time.sleep(1)

        # Reconnect and check replay
        client2 = ssh_connect()
        ch2, output2 = exec_pty(
            client2, f"attach {self.session_name}", duration=4
        )
        text2 = output2.decode("utf-8", errors="replace")

        self.assertIn(
            "RECONNECT_MARKER_42",
            text2,
            f"Replay should contain marker, got: {text2!r}",
        )

        ch2.close()
        client2.close()


class TestSSHNoPtyError(unittest.TestCase):
    """Test that PTY-requiring commands fail gracefully without PTY."""

    def test_attach_without_pty(self):
        """exec 'attach' without PTY should return error message."""
        client = ssh_connect()
        output = exec_simple(client, "attach default")
        self.assertIn(
            "PTY required",
            output,
            f"Expected 'PTY required' error, got: {output!r}",
        )
        client.close()

    def test_new_without_pty(self):
        """exec 'new' without PTY should return error message."""
        client = ssh_connect()
        output = exec_simple(client, "new no-pty-session")
        self.assertIn(
            "PTY required",
            output,
            f"Expected 'PTY required' error, got: {output!r}",
        )
        client.close()


class TestSSHExecUnknown(unittest.TestCase):
    """Test that unknown exec commands fall back to default session."""

    def test_unknown_command_with_pty(self):
        """Unknown command with PTY should connect to default session."""
        client = ssh_connect()
        channel = client.get_transport().open_session()
        channel.get_pty(term="xterm-256color", width=80, height=24)
        channel.exec_command("some-unknown-command")
        channel.settimeout(1.0)

        all_output = b""
        cpr_sent = False
        start = time.time()

        while time.time() - start < 6:
            try:
                data = channel.recv(4096)
                if not data:
                    break
                all_output += data
                if b"\x1b[6n" in data and not cpr_sent:
                    channel.send(b"\x1b[1;1R")
                    cpr_sent = True
            except Exception:
                pass

        text = all_output.decode("utf-8", errors="replace")
        # Should get a shell prompt (connected to default session)
        has_prompt = ">" in text or "PS " in text or "$" in text
        self.assertTrue(
            has_prompt,
            f"Expected shell prompt for unknown command fallback, got: {text!r}",
        )

        channel.close()
        client.close()


if __name__ == "__main__":
    # Check connectivity first
    print(f"Testing SSH server at {SSH_HOST}:{SSH_PORT}")
    try:
        client = ssh_connect()
        client.close()
        print("Connection OK\n")
    except Exception as e:
        print(f"Cannot connect to SSH server: {e}")
        print(
            f"\nMake sure the server is running with DEN_SSH_PORT={SSH_PORT}:"
        )
        print(
            '  $env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; '
            f'$env:DEN_SSH_PORT="{SSH_PORT}"; cargo run'
        )
        sys.exit(1)

    unittest.main(verbosity=2)
