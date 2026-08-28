#!/usr/bin/env python3
"""Build one CPU worker or popup model role from an immutable Git archive."""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

import gpu_model_role
from gpu_model_role import (
    ContractError,
    extract_git_archive,
    file_record,
    git_archive_commit,
    load_and_validate_spec,
    resolve_role,
    sha256_file,
    source_manifest,
    verify_rust_artifact,
)


HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")


def fail(message: str) -> None:
    raise ContractError(message)


def require_hash(value: str, length: int, label: str) -> str:
    pattern = HEX40 if length == 40 else HEX64
    if not pattern.fullmatch(value):
        fail(f"{label} must be {length} lowercase hexadecimal characters")
    return value


def run_text(argv: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(argv, env=env, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        fail(f"command failed ({result.returncode}): {' '.join(argv)}\n{result.stdout}{result.stderr}")
    return result.stdout + result.stderr


def write_text(path: Path, text: str, mode: int = 0o644) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    path.chmod(mode)


def copy_payload(source: Path, destination: Path, mode: int | None = None) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode if mode is not None else stat.S_IMODE(source.stat().st_mode))


def build_environment(target: Path) -> tuple[dict[str, str], dict[str, str]]:
    cargo = shutil.which("cargo")
    rustc = shutil.which("rustc")
    if cargo is None or rustc is None:
        fail("cargo and rustc must both be available")
    home = os.environ.get("HOME")
    if not home:
        fail("HOME must be set for the pinned Rust toolchain")
    path = os.pathsep.join(
        dict.fromkeys([str(Path(cargo).parent), str(Path(rustc).parent), "/usr/bin", "/bin"])
    )
    environment = {
        "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path(home) / ".cargo")),
        "CARGO_INCREMENTAL": "0",
        "CARGO_TARGET_DIR": str(target),
        "HOME": home,
        "LC_ALL": "C",
        "PATH": path,
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path(home) / ".rustup")),
        "TERM": "dumb",
    }
    if os.environ.get("LD_LIBRARY_PATH"):
        environment["LD_LIBRARY_PATH"] = os.environ["LD_LIBRARY_PATH"]
    return environment, {"cargo": cargo, "rustc": rustc}


def build(args: argparse.Namespace) -> Path:
    archive = args.source_archive.resolve(strict=True)
    expected_archive_sha = require_hash(args.source_archive_sha256, 64, "archive SHA-256")
    if not archive.is_file() or sha256_file(archive) != expected_archive_sha:
        fail("source archive SHA-256 mismatch")
    product_commit = require_hash(args.product_commit, 40, "product commit")
    if git_archive_commit(archive) != product_commit:
        fail("source archive belongs to a different product commit")
    builder_sha = require_hash(args.builder_sha256, 64, "builder SHA-256")
    if sha256_file(Path(__file__).resolve()) != builder_sha:
        fail("builder bytes disagree with --builder-sha256")
    contract_path = Path(gpu_model_role.__file__).resolve()
    contract_sha = require_hash(args.contract_sha256, 64, "contract SHA-256")
    if sha256_file(contract_path) != contract_sha:
        fail("model-role contract bytes disagree with --contract-sha256")
    expected_spec_sha = require_hash(args.role_spec_sha256, 64, "role-spec SHA-256")

    artifact_root = args.artifact_root.resolve()
    artifact_root.mkdir(parents=True, exist_ok=True)
    final_root = artifact_root / args.role
    staging_root = artifact_root / f".{args.role}.staging-{os.getpid()}"
    if final_root.exists() or staging_root.exists():
        fail(f"immutable role artifact already exists: {final_root}")

    try:
        with tempfile.TemporaryDirectory(prefix="quietmap-rust-role-") as temporary:
            temporary_root = Path(temporary)
            source_root = temporary_root / "source"
            target_root = temporary_root / "target"
            source_root.mkdir()
            extract_git_archive(archive, source_root)
            source_before = source_manifest(source_root)
            spec_path = source_root / "scripts/model-role-spec.json"
            if sha256_file(spec_path) != expected_spec_sha:
                fail("extracted role spec SHA-256 mismatch")
            spec = load_and_validate_spec(spec_path)
            role = resolve_role(spec, args.family, args.role)
            role_kind = spec["families"][args.family]["kind"]
            if role_kind not in {"cpu", "addon"}:
                fail("Rust role builder cannot build a GPU family")
            if sha256_file(source_root / "scripts/build-rust-model-role.py") != builder_sha:
                fail("invoked builder does not match the immutable source archive")
            if sha256_file(source_root / "scripts/gpu_model_role.py") != contract_sha:
                fail("invoked model-role contract does not match the immutable source archive")
            manifest = source_root / role["manifest"]
            if not manifest.is_file():
                fail("role Cargo manifest is absent from the source archive")

            environment, tools = build_environment(target_root)
            versions = {
                "cargo": run_text([tools["cargo"], "-vV"], environment).strip(),
                "rustc": run_text([tools["rustc"], "-vV"], environment).strip(),
            }
            host_match = re.search(r"(?m)^host:\s*(\S+)$", versions["rustc"])
            if host_match is None:
                fail("rustc -vV did not report a host triple")
            command = [
                tools["cargo"], "build", "--release", "--locked", "--manifest-path",
                str(manifest), "--no-default-features",
            ]
            receipt_command = [
                "cargo", "build", "--release", "--locked", "--manifest-path",
                role["manifest"], "--no-default-features",
            ]
            if role["cargo_features"]:
                features = ",".join(role["cargo_features"])
                command += ["--features", features]
                receipt_command += ["--features", features]
            target_args = ["--lib"] if role_kind == "addon" else ["--bin", role["binary"]]
            command += target_args
            receipt_command += target_args
            cargo_result = subprocess.run(
                command,
                cwd=source_root,
                env=environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            if cargo_result.returncode != 0:
                print(cargo_result.stdout, file=sys.stderr, end="")
                fail(f"Cargo role build failed with exit {cargo_result.returncode}")
            built = target_root / "release" / role["binary"]
            if not built.is_file() or (role_kind == "cpu" and not os.access(built, os.X_OK)):
                fail("Cargo build did not produce the exact Rust role payload")

            staging_root.mkdir(mode=0o755)
            copy_payload(built, staging_root / role["binary"], 0o755 if role_kind == "cpu" else 0o644)
            copy_payload(spec_path, staging_root / "input/model-role-spec.json", 0o644)
            copy_payload(Path(__file__).resolve(), staging_root / "input/build-rust-model-role.py", 0o755)
            copy_payload(contract_path, staging_root / "input/gpu_model_role.py", 0o755)
            write_text(staging_root / "input/source-files.sha256", source_before)
            write_text(staging_root / "logs/cargo-build.log", cargo_result.stdout)

            if source_manifest(source_root) != source_before:
                fail("the immutable extracted source changed during the build")
            lock_path = source_root / gpu_model_role.ENGINE_CARGO_LOCK
            toolchain_path = source_root / "rust-toolchain.toml"
            cargo_config_path = source_root / ".cargo/config.toml"
            if not lock_path.is_file() or not toolchain_path.is_file() or not cargo_config_path.is_file():
                fail("Rust role lock/toolchain/config input is absent")
            payload = {
                path.relative_to(staging_root).as_posix(): file_record(path)
                for path in sorted(staging_root.rglob("*")) if path.is_file()
            }
            receipt = {
                "artifact_kind": "rust-model-role",
                "binary": role["binary"],
                "build": {
                    "builder_sha256": builder_sha,
                    "cargo_command": receipt_command,
                    "contract_sha256": contract_sha,
                    "environment": {
                        "CARGO_HOME": environment["CARGO_HOME"],
                        "CARGO_INCREMENTAL": "0",
                        "CARGO_TARGET_DIR": environment["CARGO_TARGET_DIR"],
                        "HOME": environment["HOME"],
                        "LC_ALL": environment["LC_ALL"],
                        "LD_LIBRARY_PATH": environment.get("LD_LIBRARY_PATH", ""),
                        "PATH": environment["PATH"],
                        "RUSTUP_HOME": environment["RUSTUP_HOME"],
                        "TERM": environment["TERM"],
                    },
                    "fresh_target": True,
                    "rust_host": host_match.group(1),
                    "tool_versions": versions,
                },
                "cargo_features": role["cargo_features"],
                "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "family": args.family,
                "model_role": role["model_role"],
                "package": role["package"],
                "payload": payload,
                "role": args.role,
                "schema": 1,
                "selected": role["selected"],
                "source": {
                    "archive_sha256": expected_archive_sha,
                    "cargo_config_sha256": sha256_file(cargo_config_path),
                    "cargo_lock_sha256": sha256_file(lock_path),
                    "product_commit": product_commit,
                    "role_spec_sha256": expected_spec_sha,
                    "rust_toolchain_sha256": sha256_file(toolchain_path),
                    "source_manifest_sha256": sha256_file(staging_root / "input/source-files.sha256"),
                },
            }
            write_text(
                staging_root / "artifact-receipt.json",
                json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            )
            receipt_sha = sha256_file(staging_root / "artifact-receipt.json")
            write_text(
                staging_root / "BUILD_TERMINAL",
                "\n".join([
                    "RUST_MODEL_ROLE_BUILD=PASS", f"family={args.family}", f"role={args.role}",
                    f"artifact_receipt_sha256={receipt_sha}", "",
                ]),
            )
            sums = [
                f"{sha256_file(path)}  {path.relative_to(staging_root).as_posix()}"
                for path in sorted(staging_root.rglob("*"))
                if path.is_file() and path != staging_root / "SHA256SUMS"
            ]
            write_text(staging_root / "SHA256SUMS", "\n".join(sums) + "\n")
            verify_rust_artifact(staging_root, spec_path)
            staging_root.rename(final_root)
            return final_root
    except Exception:
        if staging_root.exists():
            shutil.rmtree(staging_root)
        raise


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-archive", required=True, type=Path)
    parser.add_argument("--source-archive-sha256", required=True)
    parser.add_argument("--product-commit", required=True)
    parser.add_argument("--role-spec-sha256", required=True)
    parser.add_argument("--builder-sha256", required=True)
    parser.add_argument("--contract-sha256", required=True)
    parser.add_argument("--family", required=True)
    parser.add_argument("--role", required=True)
    parser.add_argument("--artifact-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        artifact = build(args)
        print(f"RUST_MODEL_ROLE_BUILD=PASS artifact={artifact} role={args.role}")
    except (ContractError, OSError, tarfile.TarError) as error:
        print(f"RUST_MODEL_ROLE_BUILD=FAIL reason={error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
