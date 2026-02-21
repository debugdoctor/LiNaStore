#!/usr/bin/env python3
import argparse
import io
import os
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CLIENT_DIR = ROOT / "client" / "python"
if str(CLIENT_DIR) not in sys.path:
    sys.path.insert(0, str(CLIENT_DIR))

try:
    from lina_client import (
        LiNaStoreClient,
        LiNaStoreClientError,
        LiNaStoreConnectionError,
        LiNaStoreProtocolError,
        LiNaStoreChecksumError,
    )
except Exception as exc:  # pragma: no cover - import error path
    print(f"Failed to import LiNaStore python client: {exc}")
    print("Expected file: client/python/lina_client.py")
    sys.exit(2)


def run_scenario(
    label: str,
    host: str,
    port: int,
    auth_required: bool,
    username: str,
    password: str,
    data_size: int,
    timeout: int,
) -> None:
    print(f"[{label}] target={host}:{port} auth_required={auth_required}")
    client = LiNaStoreClient(host, port, timeout=timeout)

    if auth_required:
        token, expires_at = client.lina_handshake(username, password)
        print(f"[{label}] handshake ok, token expires_at={expires_at}")

    file_name = f"{label}-{uuid.uuid4().hex}"
    payload = os.urandom(data_size)

    client.lina_upload_file(file_name, io.BytesIO(payload))
    print(f"[{label}] upload ok: {file_name} ({len(payload)} bytes)")

    downloaded = client.lina_download_file(file_name)
    if downloaded != payload:
        raise AssertionError(
            f"Downloaded data mismatch for {file_name}: "
            f"expected {len(payload)} bytes, got {len(downloaded)} bytes"
        )
    print(f"[{label}] download ok: {file_name}")

    client.lina_delete_file(file_name)
    print(f"[{label}] delete ok: {file_name}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="LiNaStore advanced protocol test (python client)"
    )
    parser.add_argument(
        "--mode",
        choices=["auth-free", "auth-required", "both"],
        default="auth-free",
        help="Which auth mode(s) to test",
    )
    parser.add_argument("--host", default="127.0.0.1", help="Target host")
    parser.add_argument("--port", type=int, default=8096, help="Target port")

    parser.add_argument("--auth-free-host", default=None, help="Auth-free host")
    parser.add_argument("--auth-free-port", type=int, default=None, help="Auth-free port")
    parser.add_argument("--auth-required-host", default=None, help="Auth-required host")
    parser.add_argument(
        "--auth-required-port", type=int, default=None, help="Auth-required port"
    )

    parser.add_argument(
        "--username",
        default=os.getenv("LINASTORE_ADMIN_USER", "admin"),
        help="Admin username for auth-required mode",
    )
    parser.add_argument(
        "--password",
        default=os.getenv("LINASTORE_ADMIN_PASSWORD", "admin123"),
        help="Admin password for auth-required mode",
    )
    parser.add_argument(
        "--data-size",
        type=int,
        default=128,
        help="Payload size in bytes",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=5,
        help="Socket timeout in seconds",
    )

    return parser.parse_args()


def main() -> int:
    args = parse_args()

    scenarios = []
    if args.mode == "auth-free":
        scenarios.append(
            (
                "auth-free",
                args.host,
                args.port,
                False,
                args.username,
                args.password,
            )
        )
    elif args.mode == "auth-required":
        scenarios.append(
            (
                "auth-required",
                args.host,
                args.port,
                True,
                args.username,
                args.password,
            )
        )
    else:
        if (
            args.auth_free_host is None
            or args.auth_free_port is None
            or args.auth_required_host is None
            or args.auth_required_port is None
        ):
            print(
                "mode=both requires --auth-free-host/--auth-free-port and "
                "--auth-required-host/--auth-required-port"
            )
            return 2

        scenarios.append(
            (
                "auth-free",
                args.auth_free_host,
                args.auth_free_port,
                False,
                args.username,
                args.password,
            )
        )
        scenarios.append(
            (
                "auth-required",
                args.auth_required_host,
                args.auth_required_port,
                True,
                args.username,
                args.password,
            )
        )

    for label, host, port, auth_required, username, password in scenarios:
        try:
            run_scenario(
                label,
                host,
                port,
                auth_required,
                username,
                password,
                args.data_size,
                args.timeout,
            )
        except (
            LiNaStoreClientError,
            LiNaStoreConnectionError,
            LiNaStoreProtocolError,
            LiNaStoreChecksumError,
            AssertionError,
        ) as exc:
            print(f"[{label}] FAILED: {exc}")
            return 1

    print("All scenarios passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
