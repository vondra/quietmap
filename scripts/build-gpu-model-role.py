#!/usr/bin/env python3
"""Build one role-qualified GPU artifact from an immutable Git archive."""

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
from pathlib import Path, PurePosixPath

import gpu_model_role
from gpu_model_role import (
    ContractError,
    file_record,
    load_and_validate_spec,
    parse_nvcc_define_receipt,
    parse_ptx,
    resolve_role,
    sha256_file,
    verify_artifact,
)


CUDA_RELEASE = "13.3"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
ARCH = re.compile(r"^sm_[0-9]{2,3}$")


def fail(message: str) -> None:
    raise ContractError(message)


def require_hash(value: str, length: int, label: str) -> str:
    pattern = HEX40 if length == 40 else HEX64
    if not pattern.fullmatch(value):
        fail(f"{label} must be {length} lowercase hexadecimal characters")
    return value


def run_text(argv: list[str], env: dict[str, str] | None = None) -> str:
    result = subprocess.run(argv, env=env, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        fail(f"command failed ({result.returncode}): {' '.join(argv)}\n{result.stdout}{result.stderr}")
    return result.stdout + result.stderr


def git_archive_commit(archive: Path) -> str:
    with archive.open("rb") as source:
        result = subprocess.run(
            ["git", "get-tar-commit-id"],
            stdin=source,
            text=True,
            capture_output=True,
            check=False,
        )
    if result.returncode != 0 or not HEX40.fullmatch(result.stdout.strip()):
        fail("source archive has no exact Git commit identity")
    return result.stdout.strip()


def extract_git_archive(archive: Path, destination: Path) -> None:
    seen: set[str] = set()
    with tarfile.open(archive, mode="r:") as source:
        for member in source.getmembers():
            path = PurePosixPath(member.name)
            if (
                path.is_absolute()
                or not path.parts
                or ".." in path.parts
                or "." in path.parts
                or "\n" in member.name
                or "\r" in member.name
                or "\\" in member.name
            ):
                fail(f"source archive contains unsafe path {member.name!r}")
            relative = path.as_posix().rstrip("/")
            if relative in seen:
                fail(f"source archive contains duplicate path {relative}")
            seen.add(relative)
            target = destination.joinpath(*path.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                extracted = source.extractfile(member)
                if extracted is None:
                    fail(f"cannot read archived file {relative}")
                with target.open("xb") as output:
                    shutil.copyfileobj(extracted, output)
                target.chmod(member.mode & 0o777)
            else:
                fail(f"source archive contains non-file entry {relative}")


def source_manifest(root: Path) -> str:
    lines: list[str] = []
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        if path.is_symlink():
            fail(f"extracted source contains symlink {path.relative_to(root)}")
        if path.is_file():
            relative = path.relative_to(root).as_posix()
            lines.append(f"{sha256_file(path)}  {relative}")
    return "\n".join(lines) + "\n"


def find_one(root: Path, name: str) -> Path:
    matches = [path for path in root.rglob(name) if path.is_file()]
    if len(matches) != 1:
        fail(f"expected exactly one generated {name}, found {len(matches)}")
    return matches[0]


def write_text(path: Path, text: str, mode: int = 0o644) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    path.chmod(mode)


def copy_payload(source: Path, destination: Path, mode: int | None = None) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode if mode is not None else stat.S_IMODE(source.stat().st_mode))


def build_environment(cuda_root: Path, target: Path) -> tuple[dict[str, str], dict[str, str]]:
    cargo = shutil.which("cargo")
    rustc = shutil.which("rustc")
    if cargo is None or rustc is None:
        fail("cargo and rustc must both be available")
    nvcc = cuda_root / "bin/nvcc"
    ptxas = cuda_root / "bin/ptxas"
    if not nvcc.is_file() or not ptxas.is_file():
        fail(f"CUDA toolkit is incomplete under {cuda_root}")
    home = os.environ.get("HOME")
    if not home:
        fail("HOME must be set for the pinned Rust toolchain")
    path = os.pathsep.join(
        dict.fromkeys(
            [
                str(cuda_root / "bin"),
                str(Path(cargo).parent),
                str(Path(rustc).parent),
                "/usr/bin",
                "/bin",
            ]
        )
    )
    environment = {
        "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path(home) / ".cargo")),
        "CARGO_INCREMENTAL": "0",
        "CARGO_TARGET_DIR": str(target),
        "HOME": home,
        "LC_ALL": "C",
        "NOISE_GPU_ARCH": "",
        "NOISE_GPU_DEFINES": "",
        "PATH": path,
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path(home) / ".rustup")),
        "TERM": "dumb",
    }
    if os.environ.get("LD_LIBRARY_PATH"):
        environment["LD_LIBRARY_PATH"] = os.environ["LD_LIBRARY_PATH"]
    tools = {"cargo": cargo, "rustc": rustc, "nvcc": str(nvcc), "ptxas": str(ptxas)}
    return environment, tools


def build(args: argparse.Namespace) -> Path:
    if os.environ.get("NOISE_GPU_DEFINES", ""):
        fail("caller NOISE_GPU_DEFINES must be empty for a role artifact")
    if not ARCH.fullmatch(args.arch):
        fail("arch must have the canonical sm_NN form")
    archive = args.source_archive.resolve(strict=True)
    if not archive.is_file():
        fail("source archive is not a regular file")
    expected_archive_sha = require_hash(args.source_archive_sha256, 64, "archive SHA-256")
    if sha256_file(archive) != expected_archive_sha:
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

    cuda_root = args.cuda_root.resolve(strict=True)
    artifact_root = args.artifact_root.resolve()
    artifact_root.mkdir(parents=True, exist_ok=True)
    final_root = artifact_root / args.role
    staging_root = artifact_root / f".{args.role}.staging-{os.getpid()}"
    if final_root.exists() or staging_root.exists():
        fail(f"immutable role artifact already exists: {final_root}")

    try:
        with tempfile.TemporaryDirectory(prefix="quietmap-gpu-role-") as temporary:
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
            if sha256_file(source_root / "scripts/build-gpu-model-role.py") != builder_sha:
                fail("invoked builder does not match the immutable source archive")
            if sha256_file(source_root / "scripts/gpu_model_role.py") != contract_sha:
                fail("invoked model-role contract does not match the immutable source archive")
            manifest = source_root / role["manifest"]
            if not manifest.is_file():
                fail("role Cargo manifest is absent from the source archive")

            environment, tools = build_environment(cuda_root, target_root)
            environment["NOISE_GPU_ARCH"] = args.arch
            versions = {
                "cargo": run_text([tools["cargo"], "-vV"], environment).strip(),
                "rustc": run_text([tools["rustc"], "-vV"], environment).strip(),
                "nvcc": run_text([tools["nvcc"], "--version"], environment).strip(),
                "ptxas": run_text([tools["ptxas"], "--version"], environment).strip(),
            }
            if f"release {CUDA_RELEASE}" not in versions["nvcc"]:
                fail(f"nvcc is not the required CUDA {CUDA_RELEASE} release")
            if f"release {CUDA_RELEASE}" not in versions["ptxas"]:
                fail(f"ptxas is not the required CUDA {CUDA_RELEASE} release")

            command = [
                tools["cargo"],
                "build",
                "--release",
                "--locked",
                "--manifest-path",
                str(manifest),
                "--no-default-features",
                "--features",
                ",".join(role["cargo_features"]),
                "--bin",
                role["binary"],
            ]
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

            binary = target_root / "release" / role["binary"]
            if not binary.is_file() or not os.access(binary, os.X_OK):
                fail("Cargo build did not produce the exact executable role")
            generated_header = find_one(target_root / "release/build", "qm_streaming_abi_generated.h")
            define_receipt = find_one(target_root / "release/build", "nvcc-defines.txt")
            _, experimental_defines = parse_nvcc_define_receipt(define_receipt)
            role_noise_gpu_defines = " ".join(experimental_defines)
            if role_noise_gpu_defines:
                fail("production GPU role contains an experimental nvcc define")

            staging_root.mkdir(mode=0o755)
            copy_payload(binary, staging_root / role["binary"], 0o755)
            copy_payload(spec_path, staging_root / "input/model-role-spec.json", 0o644)
            copy_payload(Path(__file__).resolve(), staging_root / "input/build-gpu-model-role.py", 0o755)
            copy_payload(contract_path, staging_root / "input/gpu_model_role.py", 0o755)
            write_text(staging_root / "input/source-files.sha256", source_before)
            write_text(staging_root / "logs/cargo-build.log", cargo_result.stdout)
            copy_payload(
                generated_header,
                staging_root / "receipts/qm_streaming_abi_generated.h",
                0o644,
            )
            copy_payload(define_receipt, staging_root / "receipts/nvcc-defines.txt", 0o644)

            binary_bytes = binary.read_bytes()
            ptx_receipts: dict[str, dict[str, object]] = {}
            for ptx_name in role["ptx"]:
                generated_ptx = find_one(target_root / "release/build", ptx_name)
                ptx_bytes = generated_ptx.read_bytes()
                if ptx_bytes not in binary_bytes:
                    fail(f"{ptx_name} is not embedded byte-for-byte in {role['binary']}")
                ptx_target, entries = parse_ptx(ptx_bytes, ptx_name)
                if ptx_target != args.arch:
                    fail(
                        f"{ptx_name} target {ptx_target} disagrees with build arch {args.arch}"
                    )
                missing_entries = sorted(set(role["required_ptx_entries"]) - set(entries))
                if missing_entries:
                    fail(f"{ptx_name} lacks required entries {missing_entries}")
                embedded_offset = binary_bytes.find(ptx_bytes)
                copied_ptx = staging_root / "ptx" / ptx_name
                copy_payload(generated_ptx, copied_ptx, 0o644)
                cubin = temporary_root / f"{ptx_name}.cubin"
                ptxas_result = subprocess.run(
                    [tools["ptxas"], "--verbose", "--gpu-name", args.arch, str(generated_ptx), "-o", str(cubin)],
                    env=environment,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
                write_text(staging_root / "logs" / f"ptxas-{ptx_name}.log", ptxas_result.stdout)
                if ptxas_result.returncode != 0 or not cubin.is_file():
                    print(ptxas_result.stdout, file=sys.stderr, end="")
                    fail(f"ptxas resource compile failed for {ptx_name}")
                if not all(entry in ptxas_result.stdout for entry in role["required_ptx_entries"]):
                    fail(f"ptxas resource receipt omits a required entry for {ptx_name}")
                ptx_receipts[ptx_name] = {
                    "embedded_offset": embedded_offset,
                    "entries": entries,
                    "sha256": sha256_file(generated_ptx),
                }

            if source_manifest(source_root) != source_before:
                fail("the immutable extracted source changed during the build")
            lock_path = source_root / "engine/noise-gpu/Cargo.lock"
            if not lock_path.is_file():
                fail("noise-gpu Cargo.lock is absent from the source archive")
            toolchain_path = source_root / "rust-toolchain.toml"
            cargo_config_path = source_root / ".cargo/config.toml"
            if not toolchain_path.is_file() or not cargo_config_path.is_file():
                fail("Rust toolchain or Cargo config is absent from the source archive")

            payload: dict[str, dict[str, int | str]] = {}
            for path in sorted(staging_root.rglob("*")):
                if path.is_file():
                    payload[path.relative_to(staging_root).as_posix()] = file_record(path)
            receipt = {
                "artifact_kind": "gpu-model-role",
                "binary": role["binary"],
                "build": {
                    "arch": args.arch,
                    "builder_sha256": builder_sha,
                    "contract_sha256": contract_sha,
                    "cargo_command": [
                        "cargo",
                        "build",
                        "--release",
                        "--locked",
                        "--manifest-path",
                        role["manifest"],
                        "--no-default-features",
                        "--features",
                        ",".join(role["cargo_features"]),
                        "--bin",
                        role["binary"],
                    ],
                    "cuda_context": "not_opened",
                    "cuda_release": CUDA_RELEASE,
                    "cuda_root": str(cuda_root),
                    "environment": {
                        "CARGO_HOME": environment["CARGO_HOME"],
                        "CARGO_INCREMENTAL": "0",
                        "CARGO_TARGET_DIR": environment["CARGO_TARGET_DIR"],
                        "HOME": environment["HOME"],
                        "LC_ALL": environment["LC_ALL"],
                        "LD_LIBRARY_PATH": environment.get("LD_LIBRARY_PATH", ""),
                        "NOISE_GPU_ARCH": args.arch,
                        "NOISE_GPU_DEFINES": role_noise_gpu_defines,
                        "PATH": environment["PATH"],
                        "RUSTUP_HOME": environment["RUSTUP_HOME"],
                        "TERM": environment["TERM"],
                    },
                    "fresh_target": True,
                    "tool_versions": versions,
                },
                "cargo_features": role["cargo_features"],
                "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "family": args.family,
                "model_role": role["model_role"],
                "package": role["package"],
                "payload": payload,
                "ptx": ptx_receipts,
                "role": args.role,
                "schema": 1,
                "selected": role["selected"],
                "source": {
                    "archive_sha256": expected_archive_sha,
                    "cargo_lock_sha256": sha256_file(lock_path),
                    "cargo_config_sha256": sha256_file(cargo_config_path),
                    "product_commit": product_commit,
                    "role_spec_sha256": expected_spec_sha,
                    "rust_toolchain_sha256": sha256_file(toolchain_path),
                    "source_manifest_sha256": sha256_file(
                        staging_root / "input/source-files.sha256"
                    ),
                },
            }
            write_text(
                staging_root / "artifact-receipt.json",
                json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            )
            receipt_sha = sha256_file(staging_root / "artifact-receipt.json")
            write_text(
                staging_root / "BUILD_TERMINAL",
                "\n".join(
                    [
                        "GPU_MODEL_ROLE_BUILD=PASS",
                        f"family={args.family}",
                        f"role={args.role}",
                        f"artifact_receipt_sha256={receipt_sha}",
                        "",
                    ]
                ),
            )
            sums = []
            for path in sorted(staging_root.rglob("*")):
                if path.is_file() and path != staging_root / "SHA256SUMS":
                    sums.append(f"{sha256_file(path)}  {path.relative_to(staging_root).as_posix()}")
            write_text(staging_root / "SHA256SUMS", "\n".join(sums) + "\n")
            verify_artifact(staging_root, spec_path)
            # The completely verified staging directory is the publication unit.
            # Atomic rename cannot create the advisory's unverified-final state.
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
    parser.add_argument("--cuda-root", required=True, type=Path)
    parser.add_argument("--arch", required=True)
    args = parser.parse_args()
    try:
        artifact = build(args)
        print(f"GPU_MODEL_ROLE_BUILD=PASS artifact={artifact} role={args.role}")
    except (ContractError, OSError, tarfile.TarError) as error:
        print(f"GPU_MODEL_ROLE_BUILD=FAIL reason={error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
