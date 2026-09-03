#!/usr/bin/env python3
"""Validate model roles, worker-family bindings, and immutable artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tarfile
from pathlib import Path, PurePosixPath
from typing import Any


DESIGN_SHA256 = "44b606ca8fb8c5fd0b4f81d3e81c103ed0d45f495fa173d4f0760b791c939b1e"
# All eight engine crates share this lockfile (engine/Cargo.toml workspace).
ENGINE_CARGO_LOCK = "engine/Cargo.lock"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
IDENTIFIER = re.compile(r"^[a-z][a-z0-9-]*$")
ARCH = re.compile(r"^sm_[0-9]{2,3}$")
FLEET_CUDA_ARCHS_CONSTANT = re.compile(
    r'const FLEET_CUDA_ARCHS: &\[&str\] = &\[(?P<archs>.*?)\];', re.DOTALL
)
PTX_ENTRY = re.compile(r"\.visible\s+\.entry\s+([A-Za-z0-9_$]+)\s*\(")
PTX_TARGET = re.compile(r"(?m)^\s*\.target\s+(sm_[0-9]{2,3})(?:\s*,|\s*$)")
DEFINE_TOKEN = re.compile(r"^-D([A-Z][A-Z0-9_]*)(?:=(.+))?$")
GENERATED_DEFINE_NAMES = frozenset(
    {
        "AIRCRAFT_M_LAT",
        "BUILDING_ENV_CELL_STARTS",
        "BUILDING_ENV_DEM_COLS",
        "BUILDING_ENV_DEM_ELEVATION",
        "BUILDING_ENV_DEM_META",
        "BUILDING_ENV_DEM_ROWS",
        "BUILDING_ENV_DIRECTIONS",
        "BUILDING_ENV_EDGES",
        "BUILDING_ENV_EDGE_IS_BUILDING",
        "BUILDING_ENV_EDGE_REFS",
        "BUILDING_ENV_GRID_GEOMETRY",
        "BUILDING_ENV_GRID_LAYOUT",
        "BUILDING_ENV_INDEX_COUNT",
        "BUILDING_ENV_TERRAIN_SAMPLES",
        "BUILDING_FIRST_RANGE_BREAK_M_D",
        "BUILDING_GRID_GEOMETRY_STRIDE",
        "BUILDING_GRID_LAYOUT_STRIDE",
        "BUILDING_LOCAL_BANDS",
        "BUILDING_LOCAL_MAX_M_D",
        "BUILDING_LOCAL_SECTORS",
        "BUILDING_MIN_EDGE_RANGE_M_D",
        "BUILDING_RANGE_GROWTH_D",
        "BUILDING_RANGE_SCALE_D",
        "COARSE_TARGET_BLOCKS",
        "DIFFRACTION_CAP_DB_D",
        "DIFFRACTION_GRAZING_DB_D",
        "DIFFRACTION_SLOPE_D",
        "M_LAT",
        "NPD_NC",
        "SCREEN_BUILDING_LOCAL_ENTRIES",
        "SCREEN_BUILDING_LOCAL_MAX_TAN_Q",
        "SCREEN_BUILDING_GLOBAL_MAX_TAN_Q",
        "SCREEN_FAR0_BASE",
        "SCREEN_FAR0_COUNT",
        "SCREEN_FAR1_BASE",
        "SCREEN_FAR1_COUNT",
        "SCREEN_FAR2_BASE",
        "SCREEN_FAR2_COUNT",
        "SCREEN_NEAR_BASE",
        "SCREEN_NEAR_COUNT",
        "SCREEN_NREG",
        "SCREEN_RECORDS",
        "SCREEN_RECORD_OF_PIXEL",
        "SCREEN_TERRAIN_ENTRIES",
        "SCREEN_TERRAIN_MAX_SIN_SQ",
        "TAN_SCALE_D",
        "TERRAIN_BANDS",
        "TERRAIN_MARCH_SAMPLES",
        "TERRAIN_RANGE_SCALE_D",
        "TERRAIN_SECTORS",
        "TPX",
    }
)
ROLE_SHAPES = {
    "gpu-airborne": {
        "family": "airborne-production",
        "kind": "gpu",
        "manifest": "engine/noise-gpu/Cargo.toml",
        "package": "noise-gpu",
        "ptx": ["airborne.ptx"],
        "entries": [
            "airborne_classify_count",
            "airborne_classify_scatter",
            "airborne_coarse_screened",
            "airborne_exact_screened",
            "airborne_terrain_horizon_build",
            "airborne_terrain_horizon_global_max",
            "airborne_building_horizon_build",
            "airborne_building_horizon_pack",
            "airborne_building_horizon_global_max",
            "airborne_building_horizon_mark_empty",
        ],
    },
    # The production surface painter. Its CUDA is compiled to one static
    # archive linked into the binary, so it publishes no PTX of its own: the
    # binary's hash covers its whole device image.
    "relevant-source-surface": {
        "family": "relevant-source-production",
        "kind": "gpu",
        "manifest": "engine/relevant-source-gpu/Cargo.toml",
        "package": "relevant-source-gpu",
        "ptx": [],
        "entries": [],
        "cuda_image": "FLEET_CUDA_ARCHS-fatbin",
    },
    "build-heatmap-surface": {
        "family": "surface-cpu-production",
        "kind": "cpu",
        "manifest": "engine/tile-painter/Cargo.toml",
        "package": "tile-painter",
        "ptx": [],
        "entries": [],
    },
    "build-heatmap-aircraft": {
        "family": "aircraft-cpu-production",
        "kind": "cpu",
        "manifest": "engine/tile-painter/Cargo.toml",
        "package": "tile-painter",
        "ptx": [],
        "entries": [],
    },
    "libsource_reader.so": {
        "family": "popup-production",
        "kind": "addon",
        "manifest": "engine/source-reader/Cargo.toml",
        "package": "source-reader",
        "ptx": [],
        "entries": [],
    },
}


def fleet_cuda_archs(root: Path) -> list[str]:
    """Read the fleet fatbin's one architecture list from the shared build module."""
    source = (root / "engine/cuda_archs.rs").read_text(encoding="utf-8")
    matched = FLEET_CUDA_ARCHS_CONSTANT.search(source)
    if matched is None:
        raise ContractError("engine/cuda_archs.rs has no FLEET_CUDA_ARCHS constant")
    archs = re.findall(r'"(sm_[0-9]{2,3})"', matched.group("archs"))
    if not archs or len(archs) != len(set(archs)):
        raise ContractError("FLEET_CUDA_ARCHS must contain unique sm_NN architectures")
    return archs


ELF_SECTION_STRTAB = 3
ELF_SECTION_NOBITS = 8
ELF64_SECTION_HEADER_SIZE = 64
ELF_SECTION_INDEX_RESERVED = 0xFF00
NV_FATBIN_MAGIC = 0xBA55ED50
NV_FATBIN_SECTIONS = (".nv_fatbin", "__nv_relfatbin")
NV_FATBIN_CONTAINER_HEADER_MIN = 16
NV_FATBIN_ENTRY_HEADER_MIN = 32
CUDA_IMAGE_KIND_PTX = 1
CUDA_IMAGE_KIND_SASS = 2


def elf64_sections(binary_bytes: bytes) -> list[tuple[str, bytes]]:
    """Every file-backed section of a little-endian ELF64 executable, in table order,
    as (name, bytes) — a list, so a repeated name cannot hide an earlier section."""
    if binary_bytes[:6] != b"\x7fELF\x02\x01":
        raise ContractError("binary is not a little-endian ELF64 executable")
    try:
        (table_offset,) = struct.unpack_from("<Q", binary_bytes, 0x28)
        entry_size, count, names_index = struct.unpack_from("<HHH", binary_bytes, 0x3A)
        if entry_size != ELF64_SECTION_HEADER_SIZE:
            raise ContractError("binary ELF section header size is not 64")
        if count == 0 or names_index >= ELF_SECTION_INDEX_RESERVED:
            raise ContractError("binary uses extended ELF section numbering, which is unsupported")
        if table_offset + count * entry_size > len(binary_bytes):
            raise ContractError("binary ELF section table overruns the file")
        headers = [
            struct.unpack_from("<IIQQQQ", binary_bytes, table_offset + index * entry_size)
            for index in range(count)
        ]
        _, names_kind, _, _, names_offset, names_size = headers[names_index]
        if names_kind != ELF_SECTION_STRTAB or names_offset + names_size > len(binary_bytes):
            raise ContractError("binary ELF section names are not a bounded string table")
        names = binary_bytes[names_offset : names_offset + names_size]
        sections = []
        for name_offset, kind, _, _, offset, size in headers:
            if kind == ELF_SECTION_NOBITS:
                continue
            if offset + size > len(binary_bytes):
                raise ContractError("binary ELF section overruns the file")
            name = names[name_offset : names.index(b"\0", name_offset)].decode("ascii")
            sections.append((name, binary_bytes[offset : offset + size]))
    except (struct.error, IndexError, ValueError, OverflowError, UnicodeDecodeError) as error:
        raise ContractError(f"binary ELF section table is malformed: {error}") from error
    return sections


def cuda_fatbin_images(binary_bytes: bytes) -> tuple[list[str], list[str]]:
    """Read the SASS and PTX images a CUDA executable embeds, straight from its bytes.

    Layout measured on CUDA 13.3 output (r9950, 2026-09-03; identical on the
    six-image release and a single-image build): each fatbin section is a run of
    containers — u32 magic 0xBA55ED50, u16 version, u16 header size, u64 payload
    size — whose payload is a run of entries — u16 kind (1 PTX, 2 SASS cubin),
    u16 version, u32 header size, u64 padded payload size, the SM number as a
    u32 at byte 28 — each followed by its image.
    """
    fatbins = [data for name, data in elf64_sections(binary_bytes) if name in NV_FATBIN_SECTIONS]
    if not fatbins:
        raise ContractError("binary embeds no CUDA fatbin section")
    sass: list[str] = []
    ptx: list[str] = []
    for fatbin in fatbins:
        position = 0
        while position < len(fatbin):
            try:
                magic, _, header_size, size = struct.unpack_from("<IHHQ", fatbin, position)
            except struct.error as error:
                raise ContractError("CUDA fatbin container is truncated") from error
            if magic != NV_FATBIN_MAGIC:
                raise ContractError("CUDA fatbin container has the wrong magic")
            if header_size < NV_FATBIN_CONTAINER_HEADER_MIN:
                raise ContractError("CUDA fatbin container header is too small")
            entry = position + header_size
            end = entry + size
            if end > len(fatbin):
                raise ContractError("CUDA fatbin container overruns its section")
            while entry < end:
                try:
                    kind, _, entry_header_size, payload_size = struct.unpack_from(
                        "<HHIQ", fatbin, entry
                    )
                    (arch,) = struct.unpack_from("<I", fatbin, entry + 28)
                except struct.error as error:
                    raise ContractError("CUDA fatbin entry is truncated") from error
                if entry_header_size < NV_FATBIN_ENTRY_HEADER_MIN or payload_size == 0:
                    raise ContractError("CUDA fatbin entry has no image or a header too small")
                if kind == CUDA_IMAGE_KIND_SASS:
                    sass.append(f"sm_{arch}")
                elif kind == CUDA_IMAGE_KIND_PTX:
                    ptx.append(f"sm_{arch}")
                else:
                    raise ContractError(f"CUDA fatbin entry has unknown kind {kind}")
                entry += entry_header_size + payload_size
            if entry != end:
                raise ContractError("CUDA fatbin entries do not tile their container")
            position = end
    return sass, ptx


def require_fleet_fatbin(binary_bytes: bytes, source_root: Path) -> None:
    """The role builder's check of what it just built: exactly the `FLEET_CUDA_ARCHS`
    SASS images and no PTX, read from the executable itself. Admission does not
    repeat it — a wrong binary in a release is caught by the box qualification,
    which runs the painter on the card before any task (owner 2026-09-03)."""
    expected = fleet_cuda_archs(source_root)
    sass, ptx = cuda_fatbin_images(binary_bytes)
    # The fact is the image SET; nvcc's entry order is not part of it.
    if sorted(sass) != sorted(expected) or ptx:
        raise ContractError(
            f"binary embeds SASS {','.join(sass) or 'none'} and PTX {','.join(ptx) or 'none'}; "
            f"the fleet role requires SASS {','.join(expected)} and no PTX"
        )


REQUIRED_FAMILIES = frozenset(shape["family"] for shape in ROLE_SHAPES.values())
LINE_ROLE_FAMILIES = (
    "relevant-source-production",
    "surface-cpu-production",
    "popup-production",
)
MODEL_SOURCE_DIRS = (
    "engine/noise-compute",
    "engine/noise-gpu",
    "engine/relevant-source-gpu",
    "engine/source-reader",
    "engine/tile-painter",
)
MODEL_SOURCE_PRUNED_DIRS = frozenset({"target", "tests"})
# Recipe identity pinned in published generation contracts. Workspace
# [profile.release] is CODE_VER's job (layer-codever GLOBAL_BUILD), not this
# digest — hashing it here would churn live line_model_role_sha256 on a
# profile-only edit.
MODEL_SOURCE_GLOBALS = (
    ".cargo/config.toml",
    "engine/cuda_archs.rs",
    "rust-toolchain.toml",
)


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
        raise ContractError("source archive has no exact Git commit identity")
    return result.stdout.strip()


def extract_git_archive(archive: Path, destination: Path) -> None:
    seen: set[str] = set()
    symlink_paths: set[str] = set()
    extracted_symlinks: list[tuple[Path, str, str]] = []
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
                raise ContractError(f"source archive contains unsafe path {member.name!r}")
            relative = path.as_posix().rstrip("/")
            canonical_member_name = member.name.rstrip("/") if member.isdir() else member.name
            if relative != canonical_member_name:
                raise ContractError(f"source archive contains non-canonical path {member.name!r}")
            if relative in seen:
                raise ContractError(f"source archive contains duplicate path {relative}")
            for prefix_length in range(1, len(path.parts)):
                parent = PurePosixPath(*path.parts[:prefix_length]).as_posix()
                if parent in symlink_paths:
                    raise ContractError(
                        f"source archive path {relative!r} traverses symlink {parent!r}"
                    )
            seen.add(relative)
            target = destination.joinpath(*path.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                extracted = source.extractfile(member)
                if extracted is None:
                    raise ContractError(f"cannot read archived file {relative}")
                with target.open("xb") as output:
                    shutil.copyfileobj(extracted, output)
                target.chmod(member.mode & 0o777)
            elif member.issym():
                link = PurePosixPath(member.linkname)
                if (
                    link.is_absolute()
                    or not link.parts
                    or ".." in link.parts
                    or "." in link.parts
                    or link.as_posix() != member.linkname
                    or "\n" in member.linkname
                    or "\r" in member.linkname
                    or "\\" in member.linkname
                ):
                    raise ContractError(
                        f"source archive symlink {relative!r} has unsafe or non-canonical "
                        f"target {member.linkname!r}"
                    )
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.exists() or target.is_symlink():
                    raise ContractError(f"source archive symlink path already exists: {relative}")
                target.symlink_to(member.linkname)
                symlink_paths.add(relative)
                extracted_symlinks.append((target, relative, member.linkname))
            else:
                raise ContractError(f"source archive contains non-file entry {relative}")

    root = destination.resolve(strict=True)
    for target, relative, linkname in extracted_symlinks:
        try:
            resolved = target.resolve(strict=True)
            resolved.relative_to(root)
        except (OSError, RuntimeError, ValueError) as error:
            raise ContractError(
                f"source archive symlink {relative!r} does not resolve inside the source "
                f"tree: {linkname!r} ({error})"
            ) from error
        if not resolved.is_file():
            raise ContractError(
                f"source archive symlink {relative!r} does not resolve to a regular file"
            )


def source_manifest(root: Path) -> str:
    lines: list[str] = []
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        if path.is_symlink():
            relative = path.relative_to(root).as_posix()
            linkname = os.readlink(path)
            symlink_identity = hashlib.sha256(
                b"quietmap-source-symlink-v1\0" + linkname.encode("utf-8")
            ).hexdigest()
            lines.append(f"{symlink_identity}  {relative}")
        elif path.is_file():
            relative = path.relative_to(root).as_posix()
            lines.append(f"{sha256_file(path)}  {relative}")
    return "\n".join(lines) + "\n"


def parse_ptx(ptx_bytes: bytes, label: str) -> tuple[str, list[str]]:
    try:
        text = ptx_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ContractError(f"{label} is not UTF-8 PTX") from error
    targets = PTX_TARGET.findall(text)
    if len(targets) != 1:
        raise ContractError(f"{label} must contain exactly one PTX target")
    return targets[0], sorted(set(PTX_ENTRY.findall(text)))


def parse_nvcc_define_receipt(path: Path) -> list[str]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise ContractError(f"cannot read nvcc define receipt: {error}") from error
    seen: set[str] = set()
    for token in lines:
        matched = DEFINE_TOKEN.fullmatch(token)
        if matched is None:
            raise ContractError(f"invalid nvcc define receipt token {token!r}")
        name = matched.group(1)
        if name in seen:
            raise ContractError(f"duplicate nvcc define receipt macro {name}")
        seen.add(name)
        if name not in GENERATED_DEFINE_NAMES:
            raise ContractError(f"unknown nvcc define receipt macro {name}")
    return lines


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
    _exact_keys(
        spec,
        {"schema", "authority", "families", "non_worker_cohorts"},
        "spec",
    )
    if spec["schema"] != 1:
        raise ContractError("model-role spec schema must be 1")

    authority = spec["authority"]
    if not isinstance(authority, dict):
        raise ContractError("authority must be an object")
    _exact_keys(authority, {"integration_design_sha256"}, "authority")
    if authority["integration_design_sha256"] != DESIGN_SHA256:
        raise ContractError("model-role spec names the wrong integration design")

    families = spec["families"]
    if not isinstance(families, dict) or set(families) != REQUIRED_FAMILIES:
        raise ContractError("model-role spec has the wrong production-family set")

    seen_roles: set[str] = set()
    for family_name, family in families.items():
        if not IDENTIFIER.fullmatch(family_name) or not isinstance(family, dict):
            raise ContractError(f"invalid family {family_name!r}")
        _exact_keys(family, {"kind", "selected_role", "roles"}, family_name)
        if family["kind"] not in {"gpu", "cpu", "addon"}:
            raise ContractError(f"{family_name}.kind is invalid")
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
            }
            if family["kind"] == "gpu":
                required_keys |= {"ptx", "required_ptx_entries"}
            optional_keys = {
                "cuda_image",
                "selection_epoch",
            }
            if not required_keys <= set(role) or not set(role) <= required_keys | optional_keys:
                raise ContractError(f"role {role_name} has missing or unexpected fields")
            binary = role["binary"]
            shape = ROLE_SHAPES.get(binary)
            if (
                shape is None
                or shape["family"] != family_name
                or shape["kind"] != family["kind"]
            ):
                raise ContractError(f"role {role_name} binary does not belong to {family_name}")
            if role["package"] != shape["package"]:
                raise ContractError(f"role {role_name} names the wrong Cargo package")
            if role.get("cuda_image") != shape.get("cuda_image"):
                raise ContractError(f"role {role_name} has the wrong CUDA image contract")
            if _safe_relative_path(role["manifest"], f"{role_name}.manifest") != shape["manifest"]:
                raise ContractError(f"role {role_name} names the wrong Cargo manifest")
            if role.get("ptx", []) != shape["ptx"]:
                raise ContractError(f"role {role_name} has the wrong PTX set")
            expected_entries = shape["entries"]
            if role.get("required_ptx_entries", []) != expected_entries:
                raise ContractError(f"role {role_name} has the wrong PTX entry contract")

            model_role = role["model_role"]
            if model_role == "stock":
                expected_features = {
                    "gpu": ["gpu"],
                    "cpu": [],
                    "addon": ["node"],
                }[family["kind"]]
                if (
                    role["cargo_features"] != expected_features
                    or "selection_epoch" in role
                ):
                    raise ContractError(f"stock role {role_name} has non-stock features or epoch")
                if not role_name.endswith("-stock-v1"):
                    raise ContractError(f"stock role {role_name} lacks its versioned stock suffix")
            else:
                raise ContractError(f"role {role_name} has unknown model_role")

    cohorts = spec["non_worker_cohorts"]
    if not isinstance(cohorts, dict) or set(cohorts) != {"cpu-utility-stock-v1"}:
        raise ContractError("model-role spec has the wrong non-worker cohort set")
    utility = cohorts["cpu-utility-stock-v1"]
    _exact_keys(
        utility,
        {"cargo_features", "manifest", "package", "worker_launchable"},
        "cpu-utility-stock-v1",
    )
    if utility != {
        "cargo_features": [],
        "manifest": "engine/tile-painter/Cargo.toml",
        "package": "tile-painter",
        "worker_launchable": False,
    }:
        raise ContractError("cpu-utility-stock-v1 has an invalid build contract")
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


def selected_role(spec: dict[str, Any], family_name: str) -> dict[str, Any]:
    family = spec["families"].get(family_name)
    if family is None:
        raise ContractError(f"unknown artifact family {family_name!r}")
    return resolve_role(spec, family_name, family["selected_role"])


def model_role_sha256(
    spec_path: Path, spec: dict[str, Any], family_name: str, role_name: str
) -> str:
    """Hash one explicit role selection without changing the production-selected role."""
    role = resolve_role(spec, family_name, role_name)
    payload = {
        "family": family_name,
        "role": role_name,
        "role_definition": {
            key: value for key, value in role.items() if key not in {"family", "role"}
        },
        "role_spec_sha256": sha256_file(spec_path),
        "schema": 1,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def model_source_recipe_sha256(product_root: Path) -> str:
    """Hash the architecture-independent source/recipe closure of the line model.

    This intentionally over-invalidates across the four model crates. Native binaries and PTX do
    not enter this digest: their exact hashes remain artifact-cohort evidence.
    """
    records: list[tuple[str, str]] = []
    for relative_root in MODEL_SOURCE_DIRS:
        root = product_root / relative_root
        if not root.is_dir():
            raise ContractError(f"model source directory is absent: {relative_root}")
        # Cargo target/ directories are build output, never model source.
        # Prune them by name, then fail closed on any other
        # directory symlink so a relocated source dir cannot silently drop
        # files from the published recipe digest.
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            dirnames[:] = sorted(
                name for name in dirnames if name not in MODEL_SOURCE_PRUNED_DIRS
            )
            for name in dirnames:
                child = Path(dirpath) / name
                if child.is_symlink():
                    relative = child.relative_to(product_root).as_posix()
                    raise ContractError(
                        f"model source closure contains a symlink: {relative}"
                    )
            for name in sorted(filenames):
                path = Path(dirpath) / name
                relative = path.relative_to(product_root).as_posix()
                if path.is_symlink():
                    raise ContractError(f"model source closure contains a symlink: {relative}")
                if not path.is_file():
                    continue
                if path.suffix not in {".rs", ".cu", ".cuh"} and path.name != "Cargo.toml":
                    continue
                records.append((relative, sha256_file(path)))
    for relative in MODEL_SOURCE_GLOBALS:
        path = product_root / relative
        if not path.is_file() or path.is_symlink():
            raise ContractError(f"model build input is absent or unsafe: {relative}")
        records.append((relative, sha256_file(path)))
    encoded = "".join(f"{digest}  {relative}\n" for relative, digest in sorted(records)).encode()
    return hashlib.sha256(encoded).hexdigest()


def output_abi_version(product_root: Path) -> int:
    wire = product_root / "engine/tile-painter/src/wire_hm3.rs"
    match = re.search(r"(?m)^pub const VERSION: u8 = ([0-9]+);$", wire.read_text(encoding="utf-8"))
    if match is None:
        raise ContractError("cannot resolve the HM3 output ABI version")
    return int(match.group(1))


def line_model_role_sha256(
    spec_path: Path,
    spec: dict[str, Any],
    family_roles: dict[str, dict[str, Any]] | None = None,
) -> tuple[str, str, int]:
    """Architecture-independent semantic digest of the effective line-model tuple."""
    product_root = spec_path.parent.parent
    source_recipe = model_source_recipe_sha256(product_root)
    abi_version = output_abi_version(product_root)
    effective = []
    for family_name in LINE_ROLE_FAMILIES:
        role = (family_roles or {}).get(family_name) or selected_role(spec, family_name)
        effective.append(
            {
                "family": family_name,
                "model_role": role["model_role"],
                "role": role["role"],
                "selection_epoch": role.get("selection_epoch"),
            }
        )
    payload = {
        "model_source_recipe_sha256": source_recipe,
        "numerical_selection_record_sha256": None,
        "output_abi_version": abi_version,
        "role_spec_sha256": sha256_file(spec_path),
        "schema": 1,
        # Keep the version-one stock digest byte-identical. A profile-selected
        # opt-in role gets a distinct schema/key so it cannot masquerade as the
        # production selection in model-role-spec.json.
        ("selected_line_roles" if family_roles is None else "effective_line_roles"): effective,
    }
    if family_roles is not None:
        payload["schema"] = 2
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest(), source_recipe, abi_version


def _profile_role_for_family(
    spec: dict[str, Any], family_name: str, model_role: str
) -> dict[str, Any]:
    matches = [
        resolve_role(spec, family_name, role_name)
        for role_name, role in spec["families"][family_name]["roles"].items()
        if role.get("model_role") == model_role
    ]
    if len(matches) != 1:
        raise ContractError(
            f"artifact family {family_name} has {len(matches)} roles for model_role {model_role!r}"
        )
    return matches[0]


def deployment_contract(
    spec_path: Path,
    layer_spec_path: Path,
    worker_model_roles: dict[str, str] | None = None,
) -> dict[str, Any]:
    spec_path = spec_path.resolve(strict=True)
    layer_spec_path = layer_spec_path.resolve(strict=True)
    spec = load_and_validate_spec(spec_path)
    try:
        layer_spec = json.loads(layer_spec_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot read layer spec: {error}") from error
    workers = layer_spec.get("worker_types") if isinstance(layer_spec, dict) else None
    if not isinstance(workers, dict) or not workers:
        raise ContractError("layer spec has no worker_types object")
    requirements = {} if worker_model_roles is None else worker_model_roles
    if not isinstance(requirements, dict) or any(
        not isinstance(worker, str)
        or not IDENTIFIER.fullmatch(worker)
        or not isinstance(model_role, str)
        or not IDENTIFIER.fullmatch(model_role)
        for worker, model_role in requirements.items()
    ):
        raise ContractError("worker model-role requirements are invalid")
    unknown_workers = sorted(set(requirements) - set(workers))
    if unknown_workers:
        raise ContractError(
            f"worker model-role requirements name unknown workers: {','.join(unknown_workers)}"
        )
    family_roles = {
        family_name: selected_role(spec, family_name) for family_name in spec["families"]
    }
    required_family_roles: dict[str, dict[str, Any]] = {}
    for worker_name, model_role in requirements.items():
        worker = workers[worker_name]
        family_name = worker.get("artifact_family") if isinstance(worker, dict) else None
        if not isinstance(family_name, str) or family_name not in spec["families"]:
            raise ContractError(f"worker {worker_name} has no valid artifact_family")
        role = _profile_role_for_family(spec, family_name, model_role)
        previous = required_family_roles.get(family_name)
        if previous is not None and previous["role"] != role["role"]:
            raise ContractError(
                f"workers sharing {family_name} require conflicting model roles"
            )
        required_family_roles[family_name] = role
        family_roles[family_name] = role
    resolved_workers: dict[str, dict[str, Any]] = {}
    for worker_name, worker in workers.items():
        if not IDENTIFIER.fullmatch(worker_name) or not isinstance(worker, dict):
            raise ContractError(f"invalid layer-spec worker {worker_name!r}")
        family_name = worker.get("artifact_family")
        if not isinstance(family_name, str) or not IDENTIFIER.fullmatch(family_name):
            raise ContractError(f"worker {worker_name} has no valid artifact_family")
        role = family_roles[family_name]
        if worker.get("binary") != role["binary"]:
            raise ContractError(
                f"worker {worker_name} binary disagrees with effective {family_name} role"
            )
        if bool(worker.get("gpu")) != (spec["families"][family_name]["kind"] == "gpu"):
            raise ContractError(f"worker {worker_name} GPU kind disagrees with {family_name}")
        resolved_workers[worker_name] = {
            "artifact_family": family_name,
            "binary": role["binary"],
            "model_role": role["model_role"],
            "resolved_role": role["role"],
            "selection_epoch": role.get("selection_epoch"),
        }
    line_digest, source_recipe, abi_version = line_model_role_sha256(
        spec_path, spec, family_roles if requirements else None
    )
    return {
        "line_model_role_sha256": line_digest,
        "model_source_recipe_sha256": source_recipe,
        "numerical_selection_record_sha256": None,
        "output_abi_version": abi_version,
        "role_spec_sha256": sha256_file(spec_path),
        "schema": 1,
        "workers": resolved_workers,
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
    receipt_fields = {
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
        "role_sha256",
        "schema",
        "selected",
        "source",
    }
    _exact_keys(receipt, receipt_fields, "artifact receipt")
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
        ENGINE_CARGO_LOCK: source["cargo_lock_sha256"],
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
    if spec["families"][resolved["family"]]["kind"] != "gpu":
        raise ContractError("GPU artifact receipt names a non-GPU role")
    for key in ("binary", "cargo_features", "model_role", "package"):
        if receipt.get(key) != resolved[key]:
            raise ContractError(f"artifact receipt {key} disagrees with role spec")
    if receipt.get("selected") is not resolved["selected"]:
        raise ContractError("artifact receipt selected bit disagrees with role spec")
    expected_role_sha256 = model_role_sha256(
        expected_role_spec, spec, resolved["family"], resolved["role"]
    )
    if receipt.get("role_sha256") != expected_role_sha256:
        raise ContractError("artifact receipt role digest disagrees with role spec")
    fleet_cuda_image = resolved.get("cuda_image") == "FLEET_CUDA_ARCHS-fatbin"
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
    build_arch = build.get("arch")
    if (
        (fleet_cuda_image and build_arch is not None)
        or (
            not fleet_cuda_image
            and (not isinstance(build_arch, str) or not ARCH.fullmatch(build_arch))
        )
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
            "PATH",
            "RUSTUP_HOME",
            "TERM",
        },
        "artifact build environment",
    )
    if (
        environment.get("NOISE_GPU_ARCH") != ("" if fleet_cuda_image else build["arch"])
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

    parse_nvcc_define_receipt(root / "receipts/nvcc-defines.txt")

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


def verify_rust_artifact(root: Path, expected_role_spec: Path) -> dict[str, Any]:
    """Replay one CPU worker or popup role built from an immutable archive."""
    root = root.resolve(strict=True)
    records = _read_sha256sums(root)
    actual_files = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and path != root / "SHA256SUMS"
    }
    if any(path.is_symlink() for path in root.rglob("*")) or set(records) != actual_files:
        raise ContractError("Rust role artifact file set differs or contains a symlink")
    for relative, expected in records.items():
        if sha256_file(root / relative) != expected:
            raise ContractError(f"Rust role artifact hash mismatch: {relative}")
    try:
        receipt = json.loads((root / "artifact-receipt.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot read Rust role receipt: {error}") from error
    _exact_keys(
        receipt,
        {
            "artifact_kind", "binary", "build", "cargo_features", "created_at", "family",
            "model_role", "package", "payload", "role", "schema", "selected", "source",
        },
        "Rust role receipt",
    )
    if receipt["schema"] != 1 or receipt["artifact_kind"] != "rust-model-role":
        raise ContractError("invalid Rust role receipt schema")
    spec_path = root / "input/model-role-spec.json"
    spec = load_and_validate_spec(spec_path)
    expected_role_spec = expected_role_spec.resolve(strict=True)
    load_and_validate_spec(expected_role_spec)
    role = resolve_role(spec, receipt["family"], receipt["role"])
    kind = spec["families"][role["family"]]["kind"]
    if kind not in {"cpu", "addon"}:
        raise ContractError("Rust role artifact names a GPU family")
    for key in ("binary", "cargo_features", "model_role", "package"):
        if receipt[key] != role[key]:
            raise ContractError(f"Rust role receipt {key} disagrees with the role spec")
    if receipt["selected"] is not role["selected"]:
        raise ContractError("Rust role selected bit disagrees with the role spec")
    source = receipt["source"]
    _exact_keys(
        source,
        {
            "archive_sha256", "cargo_config_sha256", "cargo_lock_sha256", "product_commit",
            "role_spec_sha256", "rust_toolchain_sha256", "source_manifest_sha256",
        },
        "Rust role source receipt",
    )
    if not HEX40.fullmatch(source.get("product_commit", "")) or any(
        not HEX64.fullmatch(value) for key, value in source.items() if key != "product_commit"
    ):
        raise ContractError("Rust role source identity is incomplete")
    if source["role_spec_sha256"] != sha256_file(spec_path) or source["role_spec_sha256"] != sha256_file(expected_role_spec):
        raise ContractError("Rust role spec differs from release admission authority")
    source_manifest_path = root / "input/source-files.sha256"
    if source["source_manifest_sha256"] != sha256_file(source_manifest_path):
        raise ContractError("Rust role source manifest hash mismatch")
    source_files = _read_hash_manifest(source_manifest_path, "Rust role source manifest")
    bindings = {
        ".cargo/config.toml": source["cargo_config_sha256"],
        ENGINE_CARGO_LOCK: source["cargo_lock_sha256"],
        "rust-toolchain.toml": source["rust_toolchain_sha256"],
        "scripts/build-rust-model-role.py": sha256_file(root / "input/build-rust-model-role.py"),
        "scripts/gpu_model_role.py": sha256_file(root / "input/gpu_model_role.py"),
        "scripts/model-role-spec.json": source["role_spec_sha256"],
    }
    for relative, expected in bindings.items():
        if source_files.get(relative) != expected:
            raise ContractError(f"Rust role source binding differs: {relative}")
    build = receipt["build"]
    _exact_keys(
        build,
        {
            "builder_sha256", "cargo_command", "contract_sha256", "environment", "fresh_target",
            "rust_host", "tool_versions",
        },
        "Rust role build receipt",
    )
    if build["fresh_target"] is not True or not HEX64.fullmatch(build.get("builder_sha256", "")) \
            or not HEX64.fullmatch(build.get("contract_sha256", "")):
        raise ContractError("Rust role build identity is incomplete")
    if build["builder_sha256"] != sha256_file(root / "input/build-rust-model-role.py") \
            or build["contract_sha256"] != sha256_file(root / "input/gpu_model_role.py"):
        raise ContractError("Rust role replay helpers differ from the receipt")
    expected_command = [
        "cargo", "build", "--release", "--locked", "--manifest-path", role["manifest"],
        "--no-default-features",
    ]
    if role["cargo_features"]:
        expected_command += ["--features", ",".join(role["cargo_features"])]
    expected_command += ["--lib"] if kind == "addon" else ["--bin", role["binary"]]
    if build["cargo_command"] != expected_command:
        raise ContractError("Rust role Cargo command differs from the selected role")
    environment = build["environment"]
    _exact_keys(
        environment,
        {
            "CARGO_HOME", "CARGO_INCREMENTAL", "CARGO_TARGET_DIR", "HOME", "LC_ALL",
            "LD_LIBRARY_PATH", "PATH", "RUSTUP_HOME", "TERM",
        },
        "Rust role environment",
    )
    if environment["CARGO_INCREMENTAL"] != "0" or environment["LC_ALL"] != "C" \
            or environment["TERM"] != "dumb" or not all(environment.get(key) for key in (
                "CARGO_HOME", "CARGO_TARGET_DIR", "HOME", "PATH", "RUSTUP_HOME"
            )):
        raise ContractError("Rust role environment is incomplete")
    _exact_keys(build["tool_versions"], {"cargo", "rustc"}, "Rust role tool versions")
    if not all(isinstance(value, str) and value for value in build["tool_versions"].values()) \
            or not isinstance(build["rust_host"], str) or not build["rust_host"]:
        raise ContractError("Rust role toolchain receipt is incomplete")
    binary = root / role["binary"]
    if not binary.is_file() or (kind == "cpu" and not os.access(binary, os.X_OK)):
        raise ContractError("Rust role payload is absent or has the wrong mode")
    payload = receipt["payload"]
    if not isinstance(payload, dict) or not payload:
        raise ContractError("Rust role payload manifest is absent")
    for relative, expected in payload.items():
        relative = _safe_relative_path(relative, "Rust role payload path")
        if not isinstance(expected, dict):
            raise ContractError(f"invalid Rust role payload record: {relative}")
        _exact_keys(expected, {"bytes", "sha256"}, f"Rust role payload {relative}")
        if expected != file_record(root / relative):
            raise ContractError(f"Rust role payload mismatch: {relative}")
    if set(payload) != set(records) - {"artifact-receipt.json", "BUILD_TERMINAL"}:
        raise ContractError("Rust role payload does not cover every pre-terminal file")
    receipt_sha = sha256_file(root / "artifact-receipt.json")
    if (root / "BUILD_TERMINAL").read_text(encoding="utf-8").splitlines() != [
        "RUST_MODEL_ROLE_BUILD=PASS",
        f"family={receipt['family']}",
        f"role={receipt['role']}",
        f"artifact_receipt_sha256={receipt_sha}",
    ]:
        raise ContractError("Rust role BUILD_TERMINAL is invalid")
    return receipt


def artifact_set(
    spec_path: Path,
    layer_spec_path: Path,
    artifact_root: Path,
    families: list[str],
    worker_model_roles: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Verify effective worker-role artifacts and return their portable launch identity."""
    contract = deployment_contract(spec_path, layer_spec_path, worker_model_roles)
    spec = load_and_validate_spec(spec_path)
    unique_families = sorted(set(families))
    if not unique_families or len(unique_families) != len(families):
        raise ContractError("artifact-set families must be unique and non-empty")
    artifacts: dict[str, dict[str, Any]] = {}
    product_commit: str | None = None
    for family_name in unique_families:
        family = spec["families"].get(family_name)
        if family is None:
            raise ContractError(f"unknown artifact family {family_name!r}")
        kind = family["kind"]
        if kind == "addon":
            raise ContractError("popup artifacts are not renderer-worker artifacts")
        effective_role_names = {
            worker["resolved_role"]
            for worker in contract["workers"].values()
            if worker["artifact_family"] == family_name
        }
        if len(effective_role_names) != 1:
            raise ContractError(
                f"artifact family {family_name!r} has {len(effective_role_names)} "
                "effective worker roles"
            )
        role = resolve_role(spec, family_name, effective_role_names.pop())
        root = artifact_root / role["role"]
        receipt = (
            verify_artifact(root, spec_path)
            if kind == "gpu"
            else verify_rust_artifact(root, spec_path)
        )
        current_commit = receipt["source"]["product_commit"]
        if product_commit is None:
            product_commit = current_commit
        elif product_commit != current_commit:
            raise ContractError("artifact set mixes product commits")
        artifacts[family_name] = {
            "artifact_family": family_name,
            "artifact_manifest_sha256": sha256_file(root / "SHA256SUMS"),
            "binary": role["binary"],
            "binary_sha256": sha256_file(root / role["binary"]),
            "model_role": role["model_role"],
            "relative_binary_path": f"{role['role']}/{role['binary']}",
            "resolved_role": role["role"],
            "selection_epoch": role.get("selection_epoch"),
        }
    return {
        "artifacts": artifacts,
        "line_model_role_sha256": contract["line_model_role_sha256"],
        "model_source_recipe_sha256": contract["model_source_recipe_sha256"],
        "output_abi_version": contract["output_abi_version"],
        "product_commit": product_commit,
        "role_spec_sha256": contract["role_spec_sha256"],
        "schema": 1,
    }


def _parse_worker_model_roles_json(value: str | None) -> Any:
    if value is None:
        return None
    try:
        return json.loads(value)
    except json.JSONDecodeError as error:
        raise ContractError(
            f"worker model-role requirements are not valid JSON: {error}"
        ) from error


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate", help="validate the checked-in role spec")
    validate.add_argument("spec", type=Path)
    resolve = subparsers.add_parser("resolve", help="resolve one exact declared GPU role")
    resolve.add_argument("spec", type=Path)
    resolve.add_argument("family")
    resolve.add_argument("role")
    role_digest = subparsers.add_parser(
        "role-digest", help="print the immutable digest of one explicit declared role"
    )
    role_digest.add_argument("spec", type=Path)
    role_digest.add_argument("family")
    role_digest.add_argument("role")
    selected = subparsers.add_parser("resolve-selected", help="resolve a family's selected role")
    selected.add_argument("spec", type=Path)
    selected.add_argument("family")
    deployment = subparsers.add_parser(
        "deployment-contract",
        help="validate layer-spec artifact families and print selected worker identities",
    )
    deployment.add_argument("spec", type=Path)
    deployment.add_argument("layer_spec", type=Path)
    deployment.add_argument("--worker-model-roles-json", default=None)
    verify = subparsers.add_parser("verify-artifact", help="replay an artifact receipt")
    verify.add_argument("artifact", type=Path)
    verify.add_argument("--expected-role-spec", required=True, type=Path)
    verify_rust = subparsers.add_parser(
        "verify-rust-artifact", help="replay a CPU worker or popup artifact receipt"
    )
    verify_rust.add_argument("artifact", type=Path)
    verify_rust.add_argument("--expected-role-spec", required=True, type=Path)
    artifact_set_parser = subparsers.add_parser(
        "artifact-set", help="verify effective renderer artifacts and print launch identities"
    )
    artifact_set_parser.add_argument("spec", type=Path)
    artifact_set_parser.add_argument("layer_spec", type=Path)
    artifact_set_parser.add_argument("artifact_root", type=Path)
    artifact_set_parser.add_argument("families", nargs="+")
    artifact_set_parser.add_argument("--worker-model-roles-json", default=None)
    args = parser.parse_args()
    try:
        if args.command == "validate":
            spec = load_and_validate_spec(args.spec)
            print(f"GPU_MODEL_ROLE_SPEC=PASS families={len(spec['families'])}")
        elif args.command == "resolve":
            spec = load_and_validate_spec(args.spec)
            resolved = resolve_role(spec, args.family, args.role)
            resolved["role_sha256"] = model_role_sha256(
                args.spec, spec, args.family, args.role
            )
            print(json.dumps(resolved, sort_keys=True))
        elif args.command == "role-digest":
            spec = load_and_validate_spec(args.spec)
            print(model_role_sha256(args.spec, spec, args.family, args.role))
        elif args.command == "resolve-selected":
            spec = load_and_validate_spec(args.spec)
            print(json.dumps(selected_role(spec, args.family), sort_keys=True))
        elif args.command == "deployment-contract":
            requirements = _parse_worker_model_roles_json(
                args.worker_model_roles_json
            )
            print(json.dumps(
                deployment_contract(args.spec, args.layer_spec, requirements), sort_keys=True
            ))
        elif args.command == "verify-artifact":
            receipt = verify_artifact(args.artifact, args.expected_role_spec)
            print(f"GPU_MODEL_ROLE_ARTIFACT=PASS role={receipt['role']}")
        elif args.command == "verify-rust-artifact":
            receipt = verify_rust_artifact(args.artifact, args.expected_role_spec)
            print(f"RUST_MODEL_ROLE_ARTIFACT=PASS role={receipt['role']}")
        else:
            requirements = _parse_worker_model_roles_json(
                args.worker_model_roles_json
            )
            print(json.dumps(artifact_set(
                args.spec,
                args.layer_spec,
                args.artifact_root,
                args.families,
                requirements,
            ), sort_keys=True, separators=(",", ":")))
    except (ContractError, OSError) as error:
        print(f"GPU_MODEL_ROLE=FAIL reason={error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
