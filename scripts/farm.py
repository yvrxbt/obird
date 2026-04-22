#!/usr/bin/env python3
"""
predict.fun points-farming launcher.

Spins up one trading-cli process per TOML config in the target directory.
Monitors children, restarts on unexpected exit with exponential backoff.
Ctrl-C / SIGTERM sends SIGTERM to all children, waits up to 15s for clean
order-cancel shutdown, then SIGKILLs stragglers.

Usage (from obird workspace root):
    source .env
    python3 scripts/farm.py                          # use configs/markets_poly/
    python3 scripts/farm.py --dir <other-config-dir> # override if needed
    python3 scripts/farm.py --dry-run                # show what would start, exit

Management:
    Ctrl-C              — graceful shutdown of all markets (cancels open orders)
    kill -TERM <pid>    — same as Ctrl-C (uses SIGTERM handler)
    tail -f logs/farm/<market_id>.log    — live log for one market
    tail -f logs/farm/*.log              — all markets (interleaved)
    cat logs/farm/farm.pids              — PIDs of running child processes
"""

import argparse
import glob
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

BINARY      = "./target/release/trading-cli"
DEFAULT_DIR = "configs/markets_poly"
LOG_DIR     = "logs/farm"
PID_FILE    = "logs/farm/farm.pids"

# Crash-loop protection: if a market crashes more than MAX_RESTARTS times
# within CRASH_WINDOW_SECS, back off for CRASH_BACKOFF_SECS before retrying.
MAX_RESTARTS       = 3
CRASH_WINDOW_SECS  = 120
CRASH_BACKOFF_SECS = 300   # 5 minutes


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="predict.fun multi-market farm launcher")
    p.add_argument(
        "--dir",
        default=DEFAULT_DIR,
        metavar="DIR",
        help=f"directory containing market TOML configs (default: {DEFAULT_DIR})",
    )
    p.add_argument(
        "--dry-run",
        action="store_true",
        help="show which configs would be started and exit",
    )
    p.add_argument(
        "--skip-regen",
        action="store_true",
        help="skip the pre-flight predict-markets config regeneration",
    )
    return p.parse_args()


def regen_configs(conf_dir: str) -> None:
    """Pre-flight: refresh configs from live predict.fun + Polymarket API.

    Invariant #5: we only quote markets with a live Polymarket FV feed.
    `--fail-on-missing-poly-token` ensures any market without a resolvable
    Polymarket token is skipped (not written) rather than quoted blind.

    A non-zero exit from the binary means some markets were skipped; that's a
    warning, not a fatal error — we still want to launch whatever good configs
    got written. Fatal errors (binary missing, API unreachable) are surfaced.
    """
    print(f"[farm] Pre-flight: regenerating configs from live API → {conf_dir}/")
    # Purge stale configs so markets that expired/rotated out disappear.
    for p in glob.glob(f"{conf_dir}/*.toml"):
        if not Path(p).name.startswith("TEMPLATE"):
            Path(p).unlink()

    result = subprocess.run(
        [
            BINARY, "predict-markets",
            "--all",
            "--write-configs",
            "--fail-on-missing-poly-token",
            "--output-dir", conf_dir,
        ],
        env=os.environ.copy(),
        capture_output=True,
        text=True,
    )
    # Show the missing-poly summary line (last line of stderr on strict-mode exit).
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip().splitlines()[-1:]
        print(f"[farm]   regen warning: {tail[0] if tail else 'unknown'}")

    written = sorted(
        Path(p).name for p in glob.glob(f"{conf_dir}/*.toml")
        if not Path(p).name.startswith("TEMPLATE")
    )
    if not written:
        print(f"[farm] No configs written — is PREDICT_API_KEY set? Aborting.")
        sys.exit(1)
    print(f"[farm]   wrote {len(written)} config(s): {', '.join(written)}")


class MarketProcess:
    def __init__(self, market_id: str, conf: str):
        self.market_id       = market_id
        self.conf            = conf
        self.log_path        = f"{LOG_DIR}/{market_id}.log"
        self.proc: subprocess.Popen | None = None
        self.restart_times:  list[float] = []  # timestamps of recent restarts
        self.in_backoff      = False
        self.backoff_until   = 0.0

    def start(self) -> None:
        log_file = open(self.log_path, "a")
        self.proc = subprocess.Popen(
            [BINARY, "live", "--config", self.conf],
            stdout=log_file,
            stderr=log_file,
            env=os.environ.copy(),
        )
        print(f"[farm] {self.market_id:>10}  pid={self.proc.pid:<7}  log={self.log_path}")

    def poll(self) -> int | None:
        return self.proc.poll() if self.proc else -1

    def terminate(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()

    def kill(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.kill()

    def wait(self, timeout: float) -> None:
        if self.proc:
            self.proc.wait(timeout=timeout)

    def is_crash_looping(self) -> bool:
        """True if we've restarted too many times in CRASH_WINDOW_SECS."""
        now = time.time()
        self.restart_times = [t for t in self.restart_times if now - t < CRASH_WINDOW_SECS]
        return len(self.restart_times) >= MAX_RESTARTS

    def record_restart(self) -> None:
        self.restart_times.append(time.time())

    @property
    def pid(self) -> int:
        return self.proc.pid if self.proc else -1


def write_pid_file(markets: list[MarketProcess]) -> None:
    Path(PID_FILE).parent.mkdir(parents=True, exist_ok=True)
    with open(PID_FILE, "w") as f:
        for m in markets:
            if m.proc and m.proc.poll() is None:
                f.write(f"{m.market_id}={m.pid}\n")


def main() -> None:
    args = parse_args()
    conf_dir = args.dir.rstrip("/")

    if not Path(BINARY).exists():
        print(f"[farm] Binary not found: {BINARY}")
        print("[farm] Run:  cargo build --release --bin trading-cli")
        sys.exit(1)

    if not args.skip_regen:
        regen_configs(conf_dir)

    configs = sorted(
        p for p in glob.glob(f"{conf_dir}/*.toml")
        if not Path(p).name.startswith("TEMPLATE")
    )

    if not configs:
        print(f"[farm] No TOML configs found in {conf_dir}/")
        sys.exit(1)

    if args.dry_run:
        print(f"[farm] Dry run — would start {len(configs)} market(s) from {conf_dir}/:")
        for c in configs:
            print(f"         {Path(c).stem}")
        sys.exit(0)

    Path(LOG_DIR).mkdir(parents=True, exist_ok=True)

    markets = [MarketProcess(Path(c).stem, c) for c in configs]

    print(f"[farm] Starting {len(markets)} market(s) from {conf_dir}/")
    print(f"[farm] Binary : {BINARY}")
    print(f"[farm] Logs   : {LOG_DIR}/<market_id>.log")
    print(f"[farm] PIDs   : {PID_FILE}")
    print()

    for m in markets:
        m.start()
        time.sleep(0.5)   # stagger startup to avoid JWT auth collisions

    write_pid_file(markets)
    print(f"\n[farm] All {len(markets)} market(s) running. Ctrl-C to stop.\n")

    # ── Signal handler ────────────────────────────────────────────────────────

    def shutdown(sig, frame):
        print(f"\n[farm] Signal received — stopping all markets (graceful cancel)…")
        for m in markets:
            if m.poll() is None:
                print(f"[farm]   SIGTERM → {m.market_id} (pid {m.pid})")
                m.terminate()

        deadline = time.time() + 15
        for m in markets:
            remaining = max(0.0, deadline - time.time())
            try:
                m.wait(timeout=remaining)
                rc = m.poll()
                print(f"[farm]   {m.market_id} exited (rc={rc})")
            except subprocess.TimeoutExpired:
                print(f"[farm]   {m.market_id} still alive after 15s — SIGKILL")
                m.kill()

        # Clean up PID file
        try:
            Path(PID_FILE).unlink(missing_ok=True)
        except Exception:
            pass

        print("[farm] All markets stopped.")
        sys.exit(0)

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT,  shutdown)

    # ── Monitor loop ──────────────────────────────────────────────────────────

    last_status = time.time()

    while True:
        time.sleep(5)
        now = time.time()

        for m in markets:
            # Skip markets in backoff
            if m.in_backoff:
                if now >= m.backoff_until:
                    m.in_backoff = False
                    print(f"[farm] {m.market_id} backoff expired — restarting")
                    m.start()
                    write_pid_file(markets)
                continue

            rc = m.poll()
            if rc is None:
                continue  # still running

            # Process exited unexpectedly
            print(f"[farm] {m.market_id} exited (rc={rc})")
            m.record_restart()

            if m.is_crash_looping():
                print(
                    f"[farm] {m.market_id} crash-looping "
                    f"({MAX_RESTARTS} restarts in {CRASH_WINDOW_SECS}s) — "
                    f"backing off {CRASH_BACKOFF_SECS}s"
                )
                m.in_backoff   = True
                m.backoff_until = now + CRASH_BACKOFF_SECS
            else:
                print(f"[farm] {m.market_id} restarting in 10s…")
                time.sleep(10)
                m.start()
                write_pid_file(markets)

        # Periodic status line
        if now - last_status >= 60:
            last_status = now
            running  = sum(1 for m in markets if m.poll() is None)
            backed   = sum(1 for m in markets if m.in_backoff)
            print(f"[farm] status: {running}/{len(markets)} running, {backed} in backoff")


if __name__ == "__main__":
    main()
