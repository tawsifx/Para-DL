"""Fake macOS / subprocess execution layer backed by golden output files.

Provides deterministic responses for:
ping, traceroute, netstat, nettop, networksetup, scutil, pmset, security,
caffeinate, codesign, spctl, xattr, osascript, launchctl.

Any unmocked external tool call fails the test immediately.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

GOLDEN_DIR = Path(__file__).resolve().parent / "golden"


def read_golden(name: str) -> str:
    path = GOLDEN_DIR / name
    if path.exists():
        return path.read_text(encoding="utf-8")
    return ""


class FakeSubprocessResult:
    """Emulates subprocess.CompletedProcess."""

    def __init__(self, args: list[str], returncode: int = 0, stdout: str = "", stderr: str = "") -> None:
        self.args = args
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr


class FakePopen:
    """Emulates subprocess.Popen with non-blocking lifecycle."""

    def __init__(
        self,
        args: list[str],
        returncode: int = 0,
        stdout: str = "",
        stderr: str = "",
        on_stdin: Any = None,
    ) -> None:
        self.args = args
        self.returncode: int | None = None
        self._final_returncode = returncode
        self._stdout = stdout
        self._stderr = stderr
        self.stdin = None
        self.stdout = None
        self.stderr = None
        self.pid = 99999
        self.is_terminated = False
        self._on_stdin = on_stdin

    def poll(self) -> int | None:
        return self.returncode

    def wait(self, timeout: float | None = None) -> int:
        self.returncode = self._final_returncode
        return self.returncode

    def terminate(self) -> None:
        self.is_terminated = True
        self.returncode = 0

    def kill(self) -> None:
        self.is_terminated = True
        self.returncode = -9

    def communicate(self, input: str | bytes | None = None, timeout: float | None = None) -> tuple[str, str]:
        if input is not None and self._on_stdin:
            self._on_stdin(input)
        self.returncode = self._final_returncode
        return self._stdout, self._stderr


class MacOSFakeRunner:
    """Central runner intercepting external macOS system CLI invocations."""

    def __init__(self) -> None:
        self.recorded_calls: list[list[str]] = []
        self.keychain_store: dict[tuple[str, str], str] = {}
        self.xattr_store: dict[str, dict[str, bytes]] = {}
        self.notifications: list[dict[str, str]] = []
        self.battery_status: str = "ac"  # "ac", "battery", or "ups"
        self.battery_pct: int = 95
        self.low_power_mode: bool = False
        self.codesign_valid: bool = True
        self.spctl_valid: bool = True
        self.quarantine_attributes: dict[str, str] = {}
        self.simulated_mtu_payload: int = 1472
        self.netstat_retransmit_step: int = 0
        self._netstat_call_count: int = 0
        self.ping_rtt_override_ms: float | None = None

    def handle_call(
        self,
        args: list[str] | str,
        stdin_input: str | bytes | None = None,
        check: bool = False,
    ) -> FakeSubprocessResult:
        cmd_list = [str(a) for a in (args if isinstance(args, list | tuple) else str(args).split())]
        self.recorded_calls.append(cmd_list)
        if not cmd_list:
            return FakeSubprocessResult(cmd_list, returncode=0)

        prog = Path(cmd_list[0]).name

        # 1. pmset
        if prog == "pmset":
            if "-g" in cmd_list and "batt" in cmd_list:
                if self.battery_status == "battery":
                    out = read_golden("pmset_batt_battery.txt")
                else:
                    out = read_golden("pmset_batt_ac.txt")
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=out)
            if "-g" in cmd_list:
                out = (
                    read_golden("pmset_lowpower_on.txt")
                    if self.low_power_mode
                    else read_golden("pmset_lowpower_off.txt")
                )
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=out)
            return FakeSubprocessResult(cmd_list, returncode=0, stdout="")

        # 2. scutil
        if prog == "scutil":
            if "--nwi" in cmd_list:
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("scutil_nwi.txt"))
            return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("scutil_query_airport.txt"))

        # 3. networksetup
        if prog == "networksetup":
            return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("networksetup_airport.txt"))

        # 4. security
        if prog == "security":
            # Parsing Keychain queries and updates
            if "-i" in cmd_list:
                # Interactive mode over stdin
                script = stdin_input.decode() if isinstance(stdin_input, bytes) else (stdin_input or "")
                for line in script.splitlines():
                    if "add-generic-password" in line:
                        acc = "account"
                        srv = "pdl"
                        pw = ""
                        parts = line.split()
                        for i, p in enumerate(parts):
                            if p == "-a" and i + 1 < len(parts):
                                acc = parts[i + 1].strip('"')
                            elif p == "-s" and i + 1 < len(parts):
                                srv = parts[i + 1].strip('"')
                            elif p == "-w" and i + 1 < len(parts):
                                pw = parts[i + 1].strip('"')
                        if pw:
                            self.keychain_store[(srv, acc)] = pw
                return FakeSubprocessResult(cmd_list, returncode=0, stdout="")
            if "find-generic-password" in cmd_list:
                acc = ""
                srv = "pdl"
                for i, p in enumerate(cmd_list):
                    if p == "-a" and i + 1 < len(cmd_list):
                        acc = cmd_list[i + 1].strip('"')
                    elif p == "-s" and i + 1 < len(cmd_list):
                        srv = cmd_list[i + 1].strip('"')
                val = self.keychain_store.get((srv, acc))
                if val:
                    if "-w" in cmd_list:
                        return FakeSubprocessResult(cmd_list, returncode=0, stdout=f"{val}\n")
                    return FakeSubprocessResult(cmd_list, returncode=0, stdout=f'password: "{val}"\n')
                return FakeSubprocessResult(cmd_list, returncode=44, stderr="SecKeychainSearchCopyNext: item not found")
            if "delete-generic-password" in cmd_list:
                acc = ""
                srv = "pdl"
                for i, p in enumerate(cmd_list):
                    if p == "-a" and i + 1 < len(cmd_list):
                        acc = cmd_list[i + 1].strip('"')
                    elif p == "-s" and i + 1 < len(cmd_list):
                        srv = cmd_list[i + 1].strip('"')
                self.keychain_store.pop((srv, acc), None)
                return FakeSubprocessResult(cmd_list, returncode=0)
            return FakeSubprocessResult(cmd_list, returncode=0)

        # 5. codesign
        if prog == "codesign":
            if "--verify" in cmd_list:
                if self.codesign_valid:
                    return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("codesign_verify.txt"))
                return FakeSubprocessResult(cmd_list, returncode=1, stderr="code object is not signed at all")
            if "--sign" in cmd_list or "-s" in cmd_list:
                return FakeSubprocessResult(cmd_list, returncode=0)
            return FakeSubprocessResult(cmd_list, returncode=0)

        # 6. spctl
        if prog == "spctl":
            if self.spctl_valid:
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("spctl_assess.txt"))
            return FakeSubprocessResult(cmd_list, returncode=3, stderr="rejected")

        # 7. xattr
        if prog == "xattr":
            if "-d" in cmd_list:
                # delete attribute
                attr = cmd_list[2] if len(cmd_list) > 2 else ""
                target = cmd_list[-1]
                if target in self.xattr_store:
                    self.xattr_store[target].pop(attr, None)
                return FakeSubprocessResult(cmd_list, returncode=0)
            if "-w" in cmd_list:
                # write attribute
                is_hex = "-x" in cmd_list
                # find target path and attr
                attr = cmd_list[-3] if is_hex else cmd_list[-2]
                val = cmd_list[-2] if is_hex else cmd_list[-1]
                target = cmd_list[-1] if is_hex else cmd_list[-1]
                data = bytes.fromhex(val) if is_hex else val.encode()
                self.xattr_store.setdefault(target, {})[attr] = data
                return FakeSubprocessResult(cmd_list, returncode=0)
            if "-p" in cmd_list:
                attr = cmd_list[-2]
                target = cmd_list[-1]
                data = self.xattr_store.get(target, {}).get(attr, b"")
                if "-x" in cmd_list:
                    return FakeSubprocessResult(cmd_list, returncode=0, stdout=data.hex())
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=data.decode("utf-8", "ignore"))
            return FakeSubprocessResult(cmd_list, returncode=0)

        # 8. osascript
        if prog == "osascript":
            script = cmd_list[-1] if cmd_list else ""
            self.notifications.append({"script": script})
            return FakeSubprocessResult(cmd_list, returncode=0)

        # 9. launchctl
        if prog == "launchctl":
            if "list" in cmd_list:
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("launchctl_list.txt"))
            return FakeSubprocessResult(cmd_list, returncode=0)

        # 10. ping / traceroute / netstat / nettop
        if prog == "ping":
            if "-D" in cmd_list and "-s" in cmd_list:
                idx = cmd_list.index("-s")
                if idx + 1 < len(cmd_list):
                    try:
                        payload_sz = int(cmd_list[idx + 1])
                        if payload_sz > self.simulated_mtu_payload:
                            return FakeSubprocessResult(
                                cmd_list,
                                returncode=2,
                                stdout=read_golden("ping_df_mtu.txt"),
                            )
                    except ValueError:
                        pass
            if self.ping_rtt_override_ms is not None:
                rtt = self.ping_rtt_override_ms
                synth = (
                    f"PING 127.0.0.1 (127.0.0.1): 56 data bytes\n"
                    f"64 bytes from 127.0.0.1: icmp_seq=0 ttl=57 time={rtt:.3f} ms\n"
                    f"--- 127.0.0.1 ping statistics ---\n"
                    f"1 packets transmitted, 1 packets received, 0.0% packet loss\n"
                    f"round-trip min/avg/max/stddev = {rtt:.3f}/{rtt:.3f}/{rtt:.3f}/0.100 ms\n"
                )
                return FakeSubprocessResult(cmd_list, returncode=0, stdout=synth)
            return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("ping.txt"))
        if prog == "traceroute":
            return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("traceroute.txt"))
        if prog == "netstat":
            base_out = read_golden("netstat.txt")
            if self.netstat_retransmit_step > 0 and "-s" in cmd_list:
                step_n = self._netstat_call_count
                self._netstat_call_count += 1
                rx = 42 + step_n * self.netstat_retransmit_step
                sent = 12450 + step_n * 1000
                base_out = base_out.replace("12450 packets sent", f"{sent} packets sent").replace(
                    "42 data packets (60480 bytes) retransmitted",
                    f"{rx} data packets (60480 bytes) retransmitted",
                )
            return FakeSubprocessResult(cmd_list, returncode=0, stdout=base_out)
        if prog == "nettop":
            return FakeSubprocessResult(cmd_list, returncode=0, stdout=read_golden("nettop.txt"))

        # 11. caffeinate
        if prog == "caffeinate":
            return FakeSubprocessResult(cmd_list, returncode=0)

        # 12. plutil
        if prog == "plutil":
            return FakeSubprocessResult(cmd_list, returncode=0, stdout="plist: OK")

        # Fallback guard: any unrecognized external tool fails the test
        raise AssertionError(f"Unmocked external subprocess call: {cmd_list}")


_GLOBAL_RUNNER = MacOSFakeRunner()


def get_fake_runner() -> MacOSFakeRunner:
    return _GLOBAL_RUNNER
