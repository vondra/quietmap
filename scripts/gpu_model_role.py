#!/usr/bin/env python3
"""Validate GPU model roles and their immutable artifact receipts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from pathlib import Path, PurePosixPath
from typing import Any


DESIGN_SHA256 = "44b606ca8fb8c5fd0b4f81d3e81c103ed0d45f495fa173d4f0760b791c939b1e"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
IDENTIFIER = re.compile(r"^[a-z][a-z0-9-]*$")
ARCH = re.compile(r"^sm_[0-9]{2,3}$")
PTX_ENTRY = re.compile(r"\.visible\s+\.entry\s+([A-Za-z0-9_$]+)\s*\(")
PTX_TARGET = re.compile(r"(?m)^\s*\.target\s+(sm_[0-9]{2,3})(?:\s*,|\s*$)")
DEFINE_TOKEN = re.compile(r"^-D([A-Z][A-Z0-9_]*)(?:=(.+))?$")
EXPERIMENTAL_DEFINE_NAMES = frozenset(
    {
        "ARC_AZ_F32",
        "ARC_FOOTPRINT_CSR",
        "ARC_HULL_CACHE",
        "ARC_MAX_IV",
        "ARC_MAX_MERGED",
        "ARC_MIN_SPAN",
        "ARC_MIN_SPAN_REALISED",
        "ARC_TRI_WALK",
        "CAND_END_WINDOW_M",
        "PENUMBRA",
        "PROF_ABLATE",
        "PROF_BLOCK_MOD",
        "PROF_COUNTERS",
        "PROF_SIXMARCH",
        "SEG_ISECT_F32",
    }
)
GENERATED_DEFINE_NAMES = frozenset(
    {
        "ARC_CP_EPS",
        "ARC_DEGENERATE_SPAN",
        "ARC_FUSE_HEIGHT_TOL_M",
        "ARC_FUSE_RANGE_RATIO_LN",
        "ARC_PENUMBRA_FLOOR_M",
        "ARC_QUADRATURE_MIN_RAD",
        "BARRIER_ABI_VERSION",
        "BARRIER_STRIDE",
        "BIN_W",
        "CNOSSOS_GROUND_ALPHA0",
        "CNOSSOS_GROUND_DELTA_ZT_COEFF",
        "FOOT_BOX_STRIDE",
        "GROUND_HARD_FLOOR_DB",
        "GROUND_SOUND_SPEED",
        "LINE_KERNEL_ARGUMENT_COUNT",
        "M_LAT",
        "NPD_NC",
        "OBST_META_STRIDE",
        "P_FAV",
        "SOURCE_SEGMENT_ABI_VERSION",
        "SOURCE_SEGMENT_STRIDE",
        "SURFACE_META_ABI_VERSION",
        "SURFACE_META_SLOTS",
        "TPX",
    }
)
ROLE_SHAPES = {
    "gpu-airborne": {
        "family": "airborne-production",
        "ptx": ["airborne.ptx"],
        "entries": [
            "airborne_classify_count",
            "airborne_classify_scatter",
            "airborne_coarse_batched",
            "airborne_exact_batched",
        ],
    },
    "gpu-surface": {
        "family": "surface-production",
        "ptx": ["scatter.ptx"],
        "entries": ["line_binned_fused"],
    },
}


class ContractError(ValueError):
    """A model-role or artifact receipt violates the checked contract."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def file_record(path: Path) -> dict[str, int | str]:
    return {"bytes": path.stat().st_size, "sha256": sha256_file(path)}


def parse_ptx(ptx_bytes: bytes, label: str) -> tuple[str, list[str]]:
    try:
        text = ptx_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ContractError(f"{label} is not UTF-8 PTX") from error
    targets = PTX_TARGET.findall(text)
    if len(targets) != 1:
        raise ContractError(f"{label} must contain exactly one PTX target")
    return targets[0], sorted(set(PTX_ENTRY.findall(text)))


def parse_nvcc_define_receipt(path: Path) -> tuple[list[str], list[str]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise ContractError(f"cannot read nvcc define receipt: {error}") from error
    seen: set[str] = set()
    experimental: list[str] = []
    for token in lines:
        matched = DEFINE_TOKEN.fullmatch(token)
        if matched is None:
            raise ContractError(f"invalid nvcc define receipt token {token!r}")
        name = matched.group(1)
        if name in seen:
            raise ContractError(f"duplicate nvcc define receipt macro {name}")
        seen.add(name)
        if name in EXPERIMENTAL_DEFINE_NAMES:
            experimental.append(token)
        elif not (
            name in GENERATED_DEFINE_NAMES
            or name.startswith("V2_")
            or name.startswith("OUT_")
            or name.startswith("PROF_H0_")
            or name.startswith("H0_PAIR_DIAGNOSTIC_")
        ):
            raise ContractError(f"unknown nvcc define receipt macro {name}")
    return lines, experimental


def _exact_keys(value: dict[str, Any], expected: set[str], where: str) -> None:
    actual = set(value)
    if actual != expected:
        raise ContractError(
            f"{where} keys differ: missing={sorted(expected - actual)} "
            f"unexpected={sorted(actual - expected)}"
        )


def _safe_relative_path(value: Any, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise ContractError(f"{where} must be a non-empty relative path")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or "." in path.parts:
        raise ContractError(f"{where} is not a canonical relative path: {value!r}")
    if "\n" in value or "\r" in value or "\\" in value:
        raise ContractError(f"{where} contains an unsafe path character")
    return value


def load_and_validate_spec(path: Path) -> dict[str, Any]:
    try:
        spec = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot read model-role spec: {error}") from error
    if not isinstance(spec, dict):
        raise ContractError("model-role spec must be an object")
    _exact_keys(spec, {"schema", "authority", "families"}, "spec")
    if spec["schema"] != 1:
        raise ContractError("model-role spec schema must be 1")

    authority = spec["authority"]
    if not isinstance(authority, dict):
        raise ContractError("authority must be an object")
    _exact_keys(authority, {"integration_design_sha256"}, "authority")
    if authority["integration_design_sha256"] != DESIGN_SHA256:
        raise ContractError("model-role spec names the wrong integration design")

    families = spec["families"]
    if not isinstance(families, dict) or set(families) != {
        "airborne-production",
        "surface-production",
    }:
        raise ContractError("the GPU role spec must contain exactly both production families")

    seen_roles: set[str] = set()
    for family_name, family in families.items():
        if not IDENTIFIER.fullmatch(family_name) or not isinstance(family, dict):
            raise ContractError(f"invalid family {family_name!r}")
        _exact_keys(family, {"kind", "selected_role", "roles"}, family_name)
        if family["kind"] != "gpu":
            raise ContractError(f"{family_name}.kind must be gpu")
        roles = family["roles"]
        if not isinstance(roles, dict) or not roles:
            raise ContractError(f"{family_name}.roles must be a non-empty object")
        selected = family["selected_role"]
        if not isinstance(selected, str) or selected not in roles:
            raise ContractError(f"{family_name}.selected_role is not declared")

        for role_name, role in roles.items():
            if not IDENTIFIER.fullmatch(role_name) or role_name in seen_roles:
                raise ContractError(f"invalid or duplicate role name {role_name!r}")
            seen_roles.add(role_name)
            if not isinstance(role, dict):
                raise ContractError(f"role {role_name} must be an object")
            required_keys = {
                "binary",
                "cargo_features",
                "manifest",
                "model_role",
                "package",
                "ptx",
                "required_ptx_entries",
            }
            optional_keys = {"selection_epoch"}
            if not required_keys <= set(role) or not set(role) <= required_keys | optional_keys:
                raise ContractError(f"role {role_name} has missing or unexpected fields")
            if role["package"] != "noise-gpu":
                raise ContractError(f"role {role_name} must build package noise-gpu")
            if _safe_relative_path(role["manifest"], f"{role_name}.manifest") != (
                "engine/noise-gpu/Cargo.toml"
            ):
                raise ContractError(f"role {role_name} names the wrong Cargo manifest")

            binary = role["binary"]
            shape = ROLE_SHAPES.get(binary)
            if shape is None or shape["family"] != family_name:
                raise ContractError(f"role {role_name} binary does not belong to {family_name}")
            if role["ptx"] != shape["ptx"]:
                raise ContractError(f"role {role_name} has the wrong PTX set")
            if role["required_ptx_entries"] != shape["entries"]:
                raise ContractError(f"role {role_name} has the wrong PTX entry contract")

            model_role = role["model_role"]
            if model_role == "stock":
                if role["cargo_features"] != ["gpu"] or "selection_epoch" in role:
                    raise ContractError(f"stock role {role_name} has non-stock features or epoch")
                if not role_name.endswith("-stock-v1"):
                    raise ContractError(f"stock role {role_name} lacks its versioned stock suffix")
            elif model_role == "h0":
                raise ContractError(
                    f"H0 role {role_name} requires the pending numerical selection contract"
                )
            else:
                raise ContractError(f"role {role_name} has unknown model_role")
    return spec


def resolve_role(spec: dict[str, Any], family_name: str, role_name: str) -> dict[str, Any]:
    family = spec["families"].get(family_name)
    if family is None:
        raise ContractError(f"unknown artifact family {family_name!r}")
    role = family["roles"].get(role_name)
    if role is None:
        raise ContractError(f"role {role_name!r} is not declared in {family_name}")
    return {
        "family": family_name,
        "role": role_name,
        "selected": role_name == family["selected_role"],
        **role,
    }


def _read_hash_manifest(manifest: Path, label: str) -> dict[str, str]:
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ContractError(f"cannot read {label}: {error}") from error
    records: dict[str, str] = {}
    for line in lines:
        digest, separator, relative = line.partition("  ")
        if not separator or not HEX64.fullmatch(digest):
            raise ContractError(f"malformed {label} row: {line!r}")
        relative = _safe_relative_path(relative, f"{label} path")
        if relative in records:
            raise ContractError(f"duplicate {label} path {relative}")
        records[relative] = digest
    if list(records) != sorted(records):
        raise ContractError(f"{label} paths are not sorted")
    return records


def _read_sha256sums(root: Path) -> dict[str, str]:
    return _read_hash_manifest(root / "SHA256SUMS", "SHA256SUMS")


def verify_artifact(root: Path, expected_role_spec: Path) -> dict[str, Any]:
    root = root.resolve(strict=True)
    if not root.is_dir():
        raise ContractError("artifact root is not a directory")
    records = _read_sha256sums(root)
    actual_files: set[str] = set()
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ContractError(f"artifact contains symlink {path.relative_to(root)}")
        if path.is_file() and path != root / "SHA256SUMS":
            actual_files.add(path.relative_to(root).as_posix())
    if set(records) != actual_files:
        raise ContractError(
            f"artifact file set differs: missing={sorted(set(records) - actual_files)} "
            f"unexpected={sorted(actual_files - set(records))}"
        )
    for relative, expected in records.items():
        if sha256_file(root / relative) != expected:
            raise ContractError(f"artifact hash mismatch: {relative}")

    receipt_path = root / "artifact-receipt.json"
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot read artifact receipt: {error}") from error
    if receipt.get("schema") != 1 or receipt.get("artifact_kind") != "gpu-model-role":
        raise ContractError("invalid GPU artifact receipt schema")
    _exact_keys(
        receipt,
        {
            "artifact_kind",
            "binary",
            "build",
            "cargo_features",
            "created_at",
            "family",
            "model_role",
            "package",
            "payload",
            "ptx",
            "role",
            "schema",
            "selected",
            "source",
        },
        "artifact receipt",
    )
    source = receipt.get("source")
    if not isinstance(source, dict) or not HEX40.fullmatch(source.get("product_commit", "")):
        raise ContractError("artifact receipt has no exact product commit")
    _exact_keys(
        source,
        {
            "archive_sha256",
            "cargo_config_sha256",
            "cargo_lock_sha256",
            "product_commit",
            "role_spec_sha256",
            "rust_toolchain_sha256",
            "source_manifest_sha256",
        },
        "artifact source receipt",
    )
    for key, value in source.items():
        if key != "product_commit" and (
            not isinstance(value, str) or not HEX64.fullmatch(value)
        ):
            raise ContractError(f"artifact source receipt has invalid {key}")
    spec_path = root / "input/model-role-spec.json"
    spec = load_and_validate_spec(spec_path)
    if source.get("role_spec_sha256") != sha256_file(spec_path):
        raise ContractError("artifact receipt role-spec hash mismatch")
    expected_role_spec = expected_role_spec.resolve(strict=True)
    load_and_validate_spec(expected_role_spec)
    if source["role_spec_sha256"] != sha256_file(expected_role_spec):
        raise ContractError("artifact role spec differs from release admission authority")
    source_manifest_path = root / "input/source-files.sha256"
    if source.get("source_manifest_sha256") != sha256_file(source_manifest_path):
        raise ContractError("artifact receipt source-manifest hash mismatch")
    source_files = _read_hash_manifest(source_manifest_path, "source manifest")
    source_bindings = {
        ".cargo/config.toml": source["cargo_config_sha256"],
        "engine/noise-gpu/Cargo.lock": source["cargo_lock_sha256"],
        "rust-toolchain.toml": source["rust_toolchain_sha256"],
        "scripts/build-gpu-model-role.py": sha256_file(
            root / "input/build-gpu-model-role.py"
        ),
        "scripts/gpu_model_role.py": sha256_file(root / "input/gpu_model_role.py"),
        "scripts/model-role-spec.json": source["role_spec_sha256"],
    }
    for relative, expected in source_bindings.items():
        if source_files.get(relative) != expected:
            raise ContractError(f"source manifest disagrees with receipt input {relative}")
    resolved = resolve_role(spec, receipt.get("family", ""), receipt.get("role", ""))
    for key in ("binary", "cargo_features", "model_role", "package"):
        if receipt.get(key) != resolved[key]:
            raise ContractError(f"artifact receipt {key} disagrees with role spec")
    if receipt.get("selected") is not resolved["selected"]:
        raise ContractError("artifact receipt selected bit disagrees with role spec")
    build = receipt.get("build")
    if not isinstance(build, dict) or build.get("cuda_context") != "not_opened":
        raise ContractError("artifact receipt does not prove a compile-only CUDA build")
    _exact_keys(
        build,
        {
            "arch",
            "builder_sha256",
            "cargo_command",
            "contract_sha256",
            "cuda_context",
            "cuda_release",
            "cuda_root",
            "environment",
            "fresh_target",
            "tool_versions",
        },
        "artifact build receipt",
    )
    if (
        not ARCH.fullmatch(build.get("arch", ""))
        or build.get("cuda_release") != "13.3"
        or build.get("fresh_target") is not True
        or not HEX64.fullmatch(build.get("builder_sha256", ""))
        or not HEX64.fullmatch(build.get("contract_sha256", ""))
    ):
        raise ContractError("artifact build identity is incomplete")
    if build["builder_sha256"] != sha256_file(root / "input/build-gpu-model-role.py"):
        raise ContractError("artifact builder hash does not match its replay copy")
    if build["contract_sha256"] != sha256_file(root / "input/gpu_model_role.py"):
        raise ContractError("artifact contract hash does not match its replay copy")
    expected_command = [
        "cargo",
        "build",
        "--release",
        "--locked",
        "--manifest-path",
        resolved["manifest"],
        "--no-default-features",
        "--features",
        ",".join(resolved["cargo_features"]),
        "--bin",
        resolved["binary"],
    ]
    if build.get("cargo_command") != expected_command:
        raise ContractError("artifact Cargo command disagrees with the exact role")
    environment = build.get("environment")
    if not isinstance(environment, dict):
        raise ContractError("artifact build has no environment receipt")
    _exact_keys(
        environment,
        {
            "CARGO_HOME",
            "CARGO_INCREMENTAL",
            "CARGO_TARGET_DIR",
            "HOME",
            "LC_ALL",
            "LD_LIBRARY_PATH",
            "NOISE_GPU_ARCH",
            "NOISE_GPU_DEFINES",
            "PATH",
            "RUSTUP_HOME",
            "TERM",
        },
        "artifact build environment",
    )
    if (
        environment.get("NOISE_GPU_ARCH") != build["arch"]
        or environment.get("CARGO_INCREMENTAL") != "0"
        or environment.get("LC_ALL") != "C"
        or environment.get("TERM") != "dumb"
        or not all(
            isinstance(environment.get(key), str) and environment[key]
            for key in (
                "CARGO_HOME",
                "CARGO_TARGET_DIR",
                "HOME",
                "PATH",
                "RUSTUP_HOME",
            )
        )
        or not isinstance(environment.get("LD_LIBRARY_PATH"), str)
    ):
        raise ContractError("artifact build environment is incomplete or inconsistent")
    versions = build.get("tool_versions")
    if not isinstance(versions, dict):
        raise ContractError("artifact build has no tool-version receipt")
    _exact_keys(versions, {"cargo", "rustc", "nvcc", "ptxas"}, "tool versions")
    if not all(isinstance(value, str) and value for value in versions.values()):
        raise ContractError("artifact tool-version receipt is incomplete")

    binary_path = root / resolved["binary"]
    if not binary_path.is_file() or not os.access(binary_path, os.X_OK):
        raise ContractError("role binary is absent or not executable")
    binary_bytes = binary_path.read_bytes()
    ptx_receipt = receipt.get("ptx")
    if not isinstance(ptx_receipt, dict) or set(ptx_receipt) != set(resolved["ptx"]):
        raise ContractError("artifact receipt has the wrong PTX set")
    for ptx_name, ptx_record in ptx_receipt.items():
        if not isinstance(ptx_record, dict):
            raise ContractError(f"invalid PTX receipt for {ptx_name}")
        _exact_keys(ptx_record, {"embedded_offset", "entries", "sha256"}, ptx_name)
        ptx_path = root / "ptx" / ptx_name
        ptx_bytes = ptx_path.read_bytes()
        ptx_target, parsed_entries = parse_ptx(ptx_bytes, ptx_name)
        offset = ptx_record.get("embedded_offset")
        entries = ptx_record.get("entries")
        if (
            ptx_record.get("sha256") != sha256_file(ptx_path)
            or ptx_target != build["arch"]
            or not isinstance(entries, list)
            or not all(isinstance(entry, str) for entry in entries)
            or entries != sorted(set(entries))
            or entries != parsed_entries
            or not isinstance(offset, int)
            or offset < 0
            or binary_bytes[offset : offset + len(ptx_bytes)] != ptx_bytes
        ):
            raise ContractError(f"PTX receipt or embedded-byte proof mismatch: {ptx_name}")
        if not set(resolved["required_ptx_entries"]) <= set(entries):
            raise ContractError(f"PTX receipt lacks a required entry: {ptx_name}")
        ptxas_log = (root / "logs" / f"ptxas-{ptx_name}.log").read_text(encoding="utf-8")
        if not all(entry in ptxas_log for entry in resolved["required_ptx_entries"]):
            raise ContractError(f"ptxas receipt lacks a required entry: {ptx_name}")

    expected_v2_h0 = "1" if resolved["model_role"] == "h0" else "0"
    define_lines, experimental_defines = parse_nvcc_define_receipt(
        root / "receipts/nvcc-defines.txt"
    )
    derived_noise_gpu_defines = " ".join(experimental_defines)
    if environment["NOISE_GPU_DEFINES"] != derived_noise_gpu_defines:
        raise ContractError(
            "build environment NOISE_GPU_DEFINES disagrees with the nvcc define receipt"
        )
    if resolved["model_role"] in {"stock", "h0"} and experimental_defines:
        raise ContractError("production role contains an experimental nvcc define")
    if define_lines.count(f"-DV2_H0={expected_v2_h0}") != 1:
        raise ContractError("nvcc define receipt disagrees with the model role")
    header_lines = (
        root / "receipts/qm_streaming_abi_generated.h"
    ).read_text(encoding="utf-8").splitlines()
    if header_lines.count(f"#define V2_H0 {expected_v2_h0}") != 1:
        raise ContractError("generated host/device header disagrees with the model role")

    payload = receipt.get("payload")
    if not isinstance(payload, dict) or not payload:
        raise ContractError("artifact receipt has no payload manifest")
    for relative, expected in payload.items():
        relative = _safe_relative_path(relative, "payload path")
        if not isinstance(expected, dict):
            raise ContractError(f"invalid payload record: {relative}")
        _exact_keys(expected, {"bytes", "sha256"}, f"payload {relative}")
        if expected != file_record(root / relative):
            raise ContractError(f"payload receipt mismatch: {relative}")
    if set(payload) != set(records) - {"artifact-receipt.json", "BUILD_TERMINAL"}:
        raise ContractError("payload manifest does not cover every pre-terminal artifact file")

    receipt_sha = sha256_file(receipt_path)
    terminal = (root / "BUILD_TERMINAL").read_text(encoding="utf-8").splitlines()
    if terminal != [
        "GPU_MODEL_ROLE_BUILD=PASS",
        f"family={receipt['family']}",
        f"role={receipt['role']}",
        f"artifact_receipt_sha256={receipt_sha}",
    ]:
        raise ContractError("BUILD_TERMINAL does not authorize this artifact receipt")
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate", help="validate the checked-in role spec")
    validate.add_argument("spec", type=Path)
    resolve = subparsers.add_parser("resolve", help="resolve one exact declared GPU role")
    resolve.add_argument("spec", type=Path)
    resolve.add_argument("family")
    resolve.add_argument("role")
    verify = subparsers.add_parser("verify-artifact", help="replay an artifact receipt")
    verify.add_argument("artifact", type=Path)
    verify.add_argument("--expected-role-spec", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "validate":
            spec = load_and_validate_spec(args.spec)
            print(f"GPU_MODEL_ROLE_SPEC=PASS families={len(spec['families'])}")
        elif args.command == "resolve":
            spec = load_and_validate_spec(args.spec)
            print(json.dumps(resolve_role(spec, args.family, args.role), sort_keys=True))
        else:
            receipt = verify_artifact(args.artifact, args.expected_role_spec)
            print(f"GPU_MODEL_ROLE_ARTIFACT=PASS role={receipt['role']}")
    except (ContractError, OSError) as error:
        print(f"GPU_MODEL_ROLE=FAIL reason={error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
