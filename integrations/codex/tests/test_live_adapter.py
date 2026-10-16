from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

from terminal_process import run_in_terminal


ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / "plugins/fut-codex/scripts/fut_codex_lifecycle.py"
PROCESS_FIXTURE = Path(__file__).with_name("terminal_process.py")
FUT = os.environ.get("FUT_LIVE_BIN")


@unittest.skipUnless(FUT, "set FUT_LIVE_BIN to run against a built Fut binary")
class LiveAdapterTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.socket = root / "fut.sock"
        self.cwd = root / "workspace"
        self.cwd.mkdir()
        self.environment = os.environ.copy()
        self.environment.update(
            HOME=str(root / "home"),
            XDG_CONFIG_HOME=str(root / "config"),
            FUT_RUNTIME_DIR=str(root / "runtime"),
            TERM="xterm-256color",
            FUT_BIN=FUT,
        )
        self.daemon = subprocess.Popen(
            [
                FUT,
                "--socket",
                str(self.socket),
                "daemon",
                "run",
                "--cwd",
                str(self.cwd),
                "--",
                "/bin/sh",
                "-c",
                'while IFS= read -r line; do eval "$line"; done',
            ],
            env=self.environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            ping = subprocess.run(
                [FUT, "--socket", str(self.socket), "daemon", "ping"],
                env=self.environment,
                capture_output=True,
                check=False,
            )
            if ping.returncode == 0:
                break
            time.sleep(0.02)
        else:
            self.fail("Fut daemon did not become ready")

        resources = self.cli("list")["result"]
        self.terminal_id = resources["sessions"][0]["workspaces"][0]["tabs"][0][
            "panes"
        ][0]["terminal_id"]
        self.adapter_environment = self.environment.copy()
        self.adapter_environment.update(
            FUT_BIN=FUT,
            FUT_SOCKET=str(self.socket),
            FUT_TERMINAL_ID=self.terminal_id,
        )

    def tearDown(self) -> None:
        subprocess.run(
            [FUT, "--socket", str(self.socket), "daemon", "shutdown"],
            env=self.environment,
            capture_output=True,
            check=False,
        )
        try:
            self.daemon.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.daemon.kill()
            self.daemon.wait(timeout=3)
        self.temporary.cleanup()

    def cli(self, *arguments: str) -> dict[str, object]:
        result = subprocess.run(
            [FUT, "--socket", str(self.socket), "--json", *arguments],
            env=self.environment,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def run_report(self, command, *, input=None, attached=True, terminal_id=None):
        if not attached:
            return subprocess.run(
                command, input=input, env=self.adapter_environment,
                text=True, capture_output=True, check=False, start_new_session=True,
            )
        return self.run_in_pane(command, input=input, terminal_id=terminal_id)

    def run_in_pane(self, command, *, input=None, terminal_id=None, orphan=False):
        # Keep process behavior in the fixture: piped stdio, a separate process
        # session, and optionally reparenting outside the terminal's tree.
        result_path = Path(self.temporary.name) / f"{uuid.uuid4()}.json"
        fixture = [sys.executable, str(PROCESS_FIXTURE)]
        if orphan:
            fixture.append("--orphan")
        fixture.extend([str(result_path), json.dumps(input), json.dumps(command)])
        self.cli(
            "terminal", "run", terminal_id or self.terminal_id, shlex.join(fixture)
        )
        deadline = time.monotonic() + 5
        while not result_path.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(result_path.exists(), "report fixture did not finish")
        code, stdout, stderr = json.loads(result_path.read_text())
        return subprocess.CompletedProcess(command, code, stdout, stderr)

    def hook(self, event_name: str, *, turn_id: str | None = "turn-1", attached=True) -> None:
        event = {
            "hook_event_name": event_name,
            "session_id": "codex-session-1",
        }
        if turn_id is not None:
            event["turn_id"] = turn_id
        result = self.run_report(
            [sys.executable, str(ADAPTER), "--hook"],
            attached=attached,
            input=json.dumps(event),
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual((result.stdout, result.stderr), ("", ""))

    def notify_complete(self, turn_id: str = "turn-1", *, attached=True) -> None:
        event = json.dumps(
            {
                "type": "agent-turn-complete",
                "thread-id": "codex-session-1",
                "turn-id": turn_id,
            }
        )
        result = self.run_report(
            [FUT, "agent", "notify", "codex", event],
            attached=attached,
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual((result.stdout, result.stderr), ("", ""))

    def agent(self) -> dict[str, object]:
        return self.cli("agent", "get", self.terminal_id)["result"]["agent"]

    def resource_activity(self, terminal_id=None) -> dict[str, object]:
        terminal_id = terminal_id or self.terminal_id
        resources = self.cli("list")["result"]
        for session in resources["sessions"]:
            for workspace in session["workspaces"]:
                for tab in workspace["tabs"]:
                    for pane in tab["panes"]:
                        if pane["terminal_id"] == terminal_id:
                            return pane["activity"]
        self.fail(f"terminal {terminal_id} was absent from the resource tree")

    def assert_no_turn_metadata(self, value) -> None:
        if isinstance(value, dict):
            self.assertNotIn("turn_id", value)
            self.assertNotIn("turn-id", value)
            for child in value.values():
                self.assert_no_turn_metadata(child)
        elif isinstance(value, list):
            for child in value:
                self.assert_no_turn_metadata(child)

    def assert_activity(
        self,
        activity,
        state,
        *,
        source="codex",
        agent_session_id="codex-session-1",
    ):
        self.assertLessEqual(
            set(activity),
            {
                "records",
                "source",
                "revision",
                "working_revision",
                "attention_revision",
                "read_revision",
            },
        )
        self.assertGreater(activity["revision"], 0)
        expected_source = {"type": "report", "source": source}
        if agent_session_id is not None:
            expected_source["agent_session_id"] = agent_session_id
        self.assertEqual(activity["source"], expected_source)
        self.assertEqual(len(activity["records"]), 1)
        record = activity["records"][0]
        self.assertNotIn("id", record, "the empty root ID is omitted from JSON")
        self.assertEqual(record["state"], state)
        self.assertEqual(record["revision"], activity["revision"])
        if state == "working":
            self.assertEqual(activity["working_revision"], record["revision"])
        if state in {"done", "blocked", "error"}:
            self.assertEqual(activity["attention_revision"], record["attention_revision"])
        self.assert_no_turn_metadata(activity)
        return record

    def assert_agent(
        self,
        agent,
        state,
        *,
        available,
        unread,
        source="codex",
        agent_session_id="codex-session-1",
    ):
        self.assertEqual(
            set(agent),
            {
                "activity",
                "app",
                "available",
                "pane_id",
                "record",
                "session",
                "tab",
                "terminal_id",
                "unread",
                "workspace",
            },
        )
        record = self.assert_activity(
            agent["activity"],
            state,
            source=source,
            agent_session_id=agent_session_id,
        )
        self.assertEqual(agent["record"], record)
        self.assertEqual(agent["app"], source)
        self.assertEqual(agent["available"], available)
        self.assertEqual(agent["unread"], unread)
        self.assertEqual(agent["activity"], self.resource_activity(agent["terminal_id"]))
        return record

    def start_fake_codex(self, reports) -> Path:
        codex = Path(self.temporary.name) / "codex"
        source = codex.with_suffix(".c")
        ready = Path(self.temporary.name) / "codex-ready"
        source.write_text(
            '#include <stdlib.h>\nint main(int argc, char **argv) { return system(argv[1]); }\n'
        )
        subprocess.run(["cc", str(source), "-o", str(codex)], check=True)
        script = " && ".join(
            shlex.join([FUT, "agent", "report", *report]) for report in reports
        )
        script += f" && touch {shlex.quote(str(ready))} && sleep 2"
        self.cli("terminal", "run", self.terminal_id, shlex.join([str(codex), script]))

        deadline = time.monotonic() + 3
        while not ready.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(ready.exists(), "fake Codex process did not report")
        return ready

    def test_detached_desktop_cannot_claim_or_complete_terminal_agent(self) -> None:
        before = self.cli("list")["result"]
        self.hook("UserPromptSubmit", attached=False)
        self.notify_complete(attached=False)
        self.assertEqual(self.cli("list")["result"], before)

        # Having a controlling terminal is insufficient if it is another PTY.
        result = run_in_terminal(
            [FUT, "agent", "report", "working", "--source", "codex"],
            env=self.adapter_environment, text=True, capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "automatic agent reports must originate in the target terminal", result.stderr
        )
        self.assertEqual(self.cli("list")["result"], before)

        self.hook("UserPromptSubmit")
        self.notify_complete(attached=False)
        self.assert_agent(self.agent(), "working", available=False, unread=False)
        self.notify_complete()
        self.assert_agent(self.agent(), "done", available=True, unread=True)

        # Explicit targets remain available to external controllers.
        reported = self.cli(
            "agent", "report", "blocked",
            "--terminal-id", self.terminal_id,
            "--source", "codex",
            "--turn-id", "accepted-but-ignored",
        )["result"]
        self.assertNotIn("id", reported["report"]["value"])
        self.assertEqual(reported["source"], {"type": "report", "source": "codex"})
        self.assert_no_turn_metadata(reported)
        self.assert_agent(
            self.agent(), "blocked", available=True, unread=True,
            agent_session_id=None,
        )

        # Exited is an explicit full clear, so the terminal is no longer an agent.
        self.cli("agent", "report", "exited", "--terminal-id", self.terminal_id)
        agents = self.cli("agent", "list")["result"]["agents"]
        self.assertTrue(all(agent["terminal_id"] != self.terminal_id for agent in agents))
        activity = self.resource_activity()
        self.assertEqual(activity["records"], [])
        self.assertNotIn("source", activity)

    def test_sibling_fut_pane_cannot_report_for_another_pane(self) -> None:
        pane = self.cli("get", self.terminal_id)["result"]["target"]["pane"]["id"]
        self.cli(
            "pane", "split", pane, "right", "--", "/bin/sh", "-c",
            'while IFS= read -r line; do eval "$line"; done',
        )
        panes = self.cli("list")["result"]["sessions"][0]["workspaces"][0]["tabs"][0]["panes"]
        sibling = next(p["terminal_id"] for p in panes if p["terminal_id"] != self.terminal_id)
        self.hook("UserPromptSubmit")
        # Transfer the same root report to explicit ownership so the short-lived
        # hook fixture's PID cleanup cannot race these cross-pane assertions.
        self.cli(
            "agent", "report", "working",
            "--terminal-id", self.terminal_id,
            "--source", "codex",
            "--agent-session-id", "codex-session-1",
        )
        original = self.agent()["activity"]

        wrong_target = self.run_report(
            [
                "env", f"FUT_TERMINAL_ID={self.terminal_id}", FUT,
                "agent", "report", "completed", "--source", "codex",
            ],
            terminal_id=sibling,
        )
        self.assertNotEqual(wrong_target.returncode, 0)
        self.assertEqual(self.agent()["activity"], original)

        notification = json.dumps({"type": "agent-turn-complete", "thread-id": "codex-session-1"})
        ignored = self.run_report(
            [
                "env", f"FUT_TERMINAL_ID={self.terminal_id}", FUT,
                "agent", "notify", "codex", notification,
            ],
            terminal_id=sibling,
        )
        self.assertEqual(ignored.returncode, 0)
        self.assertEqual(self.agent()["activity"], original)

        own_target = self.run_report(
            [FUT, "agent", "report", "working", "--source", "codex"],
            terminal_id=sibling,
        )
        self.assertEqual(own_target.returncode, 0, own_target.stderr)
        sibling_agent = self.cli("agent", "get", sibling)["result"]["agent"]
        self.assert_agent(
            sibling_agent, "working", available=False, unread=False,
            agent_session_id=None,
        )
        self.assertEqual(self.agent()["activity"], original)

    def test_reparented_process_with_real_inherited_context_cannot_report(self) -> None:
        self.hook("SessionStart")
        original = self.agent()["activity"]
        # Inherit real Fut context, then outlive the launcher as a desktop app does.
        result = self.run_in_pane(
            [FUT, "agent", "report", "working", "--source", "codex"],
            orphan=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "automatic agent reports must originate in the target terminal", result.stderr
        )
        self.assertEqual(self.agent()["activity"], original)

    def test_agent_integration_exits_with_its_reporting_codex_process(self) -> None:
        self.start_fake_codex([("working", "--source", "codex")])
        working = self.agent()
        self.assert_agent(
            working, "working", available=False, unread=False,
            agent_session_id=None,
        )
        working_revision = working["activity"]["revision"]

        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            agents = self.cli("agent", "list")["result"]["agents"]
            if all(agent["terminal_id"] != self.terminal_id for agent in agents):
                break
            time.sleep(0.05)
        else:
            self.fail("exited Codex process remained an active agent")
        activity = self.resource_activity()
        self.assertEqual(activity["records"], [])
        self.assertNotIn("source", activity)
        self.assertGreater(activity["revision"], working_revision)

    def test_automatic_pid_cleanup_is_scoped_and_preserves_results(self) -> None:
        self.start_fake_codex(
            [
                ("working", "--source", "codex"),
                ("working", "--id", "automatic", "--source", "codex"),
                ("done", "--id", "result", "--source", "codex"),
                ("error", "--id", "failure", "--source", "codex"),
            ]
        )
        self.cli(
            "agent", "report", "working",
            "--terminal-id", self.terminal_id,
            "--id", "external",
            "--source", "controller",
        )

        deadline = time.monotonic() + 5
        activity = self.resource_activity()
        while time.monotonic() < deadline:
            records = {record.get("id", ""): record for record in activity["records"]}
            if "" not in records and "automatic" not in records:
                break
            time.sleep(0.05)
            activity = self.resource_activity()
        else:
            self.fail("exited automatic reporter retained transient records")

        self.assertEqual(set(records), {"result", "failure", "external"})
        self.assertEqual(records["result"]["state"], "done")
        self.assertEqual(records["failure"]["state"], "error")
        self.assertEqual(records["external"]["state"], "working")
        agent = self.agent()
        self.assertEqual(agent["record"], records["failure"])
        self.assertEqual(agent["app"], "controller")
        self.assertFalse(agent["available"])
        self.assertTrue(agent["unread"])

        self.cli("agent", "report", "exited", "--terminal-id", self.terminal_id)
        agents = self.cli("agent", "list")["result"]["agents"]
        self.assertTrue(all(agent["terminal_id"] != self.terminal_id for agent in agents))
        cleared = self.resource_activity()
        self.assertEqual(cleared["records"], [])
        self.assertNotIn("source", cleared)

    def test_real_report_get_and_wait_surface_observes_native_transitions(self) -> None:
        self.hook("SessionStart", turn_id=None)
        agent = self.agent()
        self.assert_agent(agent, "idle", available=True, unread=False)
        self.assertNotIn("working_revision", agent["activity"])
        self.assertNotIn("attention_revision", agent["activity"])
        self.assertNotIn("read_revision", agent["activity"])

        self.hook("UserPromptSubmit")
        agent = self.agent()
        self.assert_agent(agent, "working", available=False, unread=False)
        self.assertEqual(
            agent["activity"]["working_revision"],
            agent["record"]["revision"],
        )

        waiter = subprocess.Popen(
            [
                FUT,
                "--socket",
                str(self.socket),
                "--json",
                "agent",
                "wait",
                self.terminal_id,
                "--timeout",
                "3s",
            ],
            env=self.environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.notify_complete()
        stdout, stderr = waiter.communicate(timeout=4)
        self.assertEqual(waiter.returncode, 0, stderr)
        settled = json.loads(stdout)["result"]
        settled_record = self.assert_activity(settled["activity"], "done")
        self.assertEqual(settled["activity"], self.resource_activity())
        self.assertEqual(
            settled["working_revision"], settled["activity"]["working_revision"]
        )
        self.assertLessEqual(settled["working_revision"], settled["barrier_revision"])
        self.assertLess(settled["barrier_revision"], settled_record["revision"])

        prompted = subprocess.Popen(
            [
                FUT,
                "--socket",
                str(self.socket),
                "--json",
                "agent",
                "prompt",
                self.terminal_id,
                "CODEX_LIVE_PROMPT",
                "--wait",
                "--timeout",
                "3s",
            ],
            env=self.environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.cli(
            "terminal",
            "wait-output",
            self.terminal_id,
            "--literal",
            "CODEX_LIVE_PROMPT",
            "--timeout",
            "2s",
        )
        self.hook("UserPromptSubmit", turn_id="turn-prompt")
        self.notify_complete(turn_id="turn-prompt")
        stdout, stderr = prompted.communicate(timeout=4)
        self.assertEqual(prompted.returncode, 0, stderr)
        prompt_result = json.loads(stdout)["result"]
        self.assertTrue(prompt_result["submitted"])
        self.assertIsNotNone(prompt_result["working_revision"])
        prompt_record = self.assert_activity(prompt_result["activity"], "done")
        self.assertEqual(
            prompt_result["working_revision"],
            prompt_result["activity"]["working_revision"],
        )
        self.assertLess(prompt_result["barrier_revision"], prompt_result["working_revision"])
        self.assertLess(prompt_result["working_revision"], prompt_record["revision"])
        self.assertEqual(prompt_result["activity"], self.resource_activity())

        self.hook("UserPromptSubmit", turn_id="turn-2")
        self.hook("PermissionRequest", turn_id="turn-2")
        blocked_result = self.cli(
            "agent", "wait", self.terminal_id, "--timeout", "1s"
        )["result"]
        blocked = blocked_result["activity"]
        blocked_record = self.assert_activity(blocked, "blocked")
        self.assertEqual(blocked, self.resource_activity())
        self.assertIsNone(blocked_result["working_revision"])
        self.assertLess(blocked["working_revision"], blocked_record["revision"])
        self.assert_agent(self.agent(), "blocked", available=True, unread=True)


if __name__ == "__main__":
    unittest.main()
