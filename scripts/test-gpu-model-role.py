#!/usr/bin/env python3
"""Mutation and fake-toolchain tests for production model-role artifacts."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

from gpu_model_role import (
    MODEL_SOURCE_DIRS,
    ContractError,
    artifact_set,
    cuda_fatbin_images,
    deployment_contract,
    load_and_validate_spec,
    model_source_recipe_sha256,
    model_role_sha256,
    relevant_source_fleet_cuda_archs,
    resolve_role,
    verify_artifact,
    verify_rust_artifact,
)


ROOT = Path(__file__).resolve().parent.parent
SPEC_PATH = ROOT / "scripts/model-role-spec.json"
BUILDER = ROOT / "scripts/build-gpu-model-role.py"
RUST_BUILDER = ROOT / "scripts/build-rust-model-role.py"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# Fake CUDA executable: a minimal ELF64 whose `.nv_fatbin` carries fake SASS and
# PTX entries in the measured CUDA 13.3 layout. The fake toolchain runs this file
# as a program; the tests here exec it for the same function.
FAKE_CUDA_EXECUTABLE_SOURCE = '''\
import struct
import sys

FATBIN_MAGIC = 0xBA55ED50


def fatbin_entry(kind, arch):
    payload = b"fake image %d %d" % (kind, arch)
    payload += b"\\0" * (-len(payload) % 8)
    header = struct.pack("<HHIQ", kind, 0x0101, 64, len(payload))
    header += b"\\0" * 12 + struct.pack("<I", arch)
    return header + b"\\0" * (64 - len(header)) + payload


def fatbin_section(sass, ptx):
    entries = b"".join(fatbin_entry(2, arch) for arch in sass)
    entries += b"".join(fatbin_entry(1, arch) for arch in ptx)
    return struct.pack("<IHHQ", FATBIN_MAGIC, 1, 16, len(entries)) + entries


def fake_cuda_executable(sass, ptx, *more_fatbins):
    """One `.nv_fatbin` section, plus one more per extra (sass, ptx) pair."""
    fatbins = [fatbin_section(sass, ptx)] + [fatbin_section(s, p) for s, p in more_fatbins]
    names = b"\\0.nv_fatbin\\0.shstrtab\\0"

    def section(name, offset, size, kind=1):
        return struct.pack("<IIQQQQIIQQ", name, kind, 0, 0, offset, size, 0, 0, 1, 0)

    body = b""
    sections = b"\\0" * 64
    for fatbin in fatbins:
        sections += section(1, 64 + len(body), len(fatbin))
        body += fatbin
    names_offset = 64 + len(body)
    headers_offset = names_offset + len(names)
    count = 2 + len(fatbins)
    header = b"\\x7fELF\\x02\\x01\\x01" + b"\\0" * 9
    header += struct.pack(
        "<HHIQQQIHHHHHH", 2, 62, 1, 0, 0, headers_offset, 0, 64, 0, 0, 64, count, count - 1
    )
    sections += section(12, names_offset, len(names), 3)
    return header + body + names + sections


if __name__ == "__main__":
    archs = lambda text: [int(arch.removeprefix("sm_")) for arch in text.split(",") if arch]
    with open(sys.argv[1], "wb") as out:
        out.write(fake_cuda_executable(archs(sys.argv[2]), archs(sys.argv[3])))
'''
_fake_namespace: dict[str, object] = {}
exec(FAKE_CUDA_EXECUTABLE_SOURCE, _fake_namespace)
fake_cuda_executable = _fake_namespace["fake_cuda_executable"]


def write_executable(path: Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(0o755)


def write_model_source_fixture(root: Path) -> Path:
    """A minimal product tree covering the whole model-source closure, derived
    from MODEL_SOURCE_DIRS so a new model crate cannot leave the fixture behind.
    Returns the CUDA header the digest tests mutate."""
    for relative in MODEL_SOURCE_DIRS:
        (root / relative).mkdir(parents=True, exist_ok=True)
        (root / relative / "Cargo.toml").write_text(
            "[package]\nname='fixture'\n", encoding="utf-8"
        )
    header = root / "engine/noise-gpu/kernels/qm_fixture.cuh"
    header.parent.mkdir(parents=True, exist_ok=True)
    header.write_text("#define FIXTURE 1\n", encoding="utf-8")
    (root / ".cargo").mkdir(parents=True, exist_ok=True)
    (root / ".cargo/config.toml").write_text("[build]\n", encoding="utf-8")
    (root / "rust-toolchain.toml").write_text("[toolchain]\n", encoding="utf-8")
    return header


def reseal_artifact(artifact: Path) -> None:
    receipt_path = artifact / "artifact-receipt.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    for relative in receipt["payload"]:
        path = artifact / relative
        receipt["payload"][relative] = {"bytes": path.stat().st_size, "sha256": sha256(path)}
    receipt_path.write_text(
        json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    (artifact / "BUILD_TERMINAL").write_text(
        "\n".join(
            [
                "GPU_MODEL_ROLE_BUILD=PASS",
                f"family={receipt['family']}",
                f"role={receipt['role']}",
                f"artifact_receipt_sha256={sha256(receipt_path)}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    lines = []
    for path in sorted(artifact.rglob("*")):
        if path.is_file() and path != artifact / "SHA256SUMS":
            lines.append(f"{sha256(path)}  {path.relative_to(artifact).as_posix()}")
    (artifact / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


class ModelRoleSpecTests(unittest.TestCase):
    def setUp(self) -> None:
        self.spec = json.loads(SPEC_PATH.read_text(encoding="utf-8"))

    def validate_mutation(self, mutate) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "spec.json"
            changed = copy.deepcopy(self.spec)
            mutate(changed)
            path.write_text(json.dumps(changed), encoding="utf-8")
            with self.assertRaises(ContractError):
                load_and_validate_spec(path)

    def test_production_roles_are_stock_and_surface_uses_relevant_source(self) -> None:
        spec = load_and_validate_spec(SPEC_PATH)
        airborne = resolve_role(spec, "airborne-production", "airborne-stock-v1")
        self.assertTrue(airborne["selected"])
        relevant_source = resolve_role(
            spec, "relevant-source-production", "relevant-source-stock-v1"
        )
        self.assertTrue(relevant_source["selected"])
        self.assertEqual(relevant_source["binary"], "relevant-source-surface")
        self.assertRegex(
            model_role_sha256(SPEC_PATH, spec, relevant_source["family"], relevant_source["role"]),
            r"^[0-9a-f]{64}$",
        )
        self.assertEqual(airborne["cargo_features"], ["gpu"])
        self.assertEqual(relevant_source["cuda_image"], "FLEET_CUDA_ARCHS-fatbin")
        fleet_archs = relevant_source_fleet_cuda_archs(ROOT)
        self.assertTrue(all(re.fullmatch(r"sm_[0-9]{2,3}", arch) for arch in fleet_archs))
        self.assertEqual(
            resolve_role(spec, "surface-cpu-production", "surface-cpu-stock-v1")[
                "binary"
            ],
            "build-heatmap-surface",
        )
        self.assertEqual(
            resolve_role(spec, "popup-production", "popup-stock-v1")["cargo_features"],
            ["node"],
        )

    def test_every_layer_worker_resolves_one_selected_artifact_family(self) -> None:
        contract = deployment_contract(SPEC_PATH, ROOT / "scripts/layer-spec.json")
        layer_spec = json.loads((ROOT / "scripts/layer-spec.json").read_text(encoding="utf-8"))
        self.assertEqual(set(contract["workers"]), set(layer_spec["worker_types"]))
        self.assertRegex(contract["line_model_role_sha256"], r"^[0-9a-f]{64}$")
        self.assertRegex(contract["model_source_recipe_sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(contract["output_abi_version"], 3)

    def test_profile_requirements_reject_falsy_non_objects_in_api_and_cli(self) -> None:
        for invalid in ([], False, 0, ""):
            with self.subTest(value=invalid):
                with self.assertRaisesRegex(
                    ContractError, "worker model-role requirements are invalid"
                ):
                    deployment_contract(
                        SPEC_PATH, ROOT / "scripts/layer-spec.json", invalid
                    )
                cli = subprocess.run(
                    [
                        sys.executable,
                        str(ROOT / "scripts/gpu_model_role.py"),
                        "deployment-contract",
                        str(SPEC_PATH),
                        str(ROOT / "scripts/layer-spec.json"),
                        "--worker-model-roles-json",
                        json.dumps(invalid),
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
                self.assertNotEqual(cli.returncode, 0, cli.stdout + cli.stderr)
                self.assertIn(
                    "worker model-role requirements are invalid", cli.stderr
                )

    def test_layer_worker_family_or_binary_mutation_is_rejected(self) -> None:
        layer_spec = json.loads((ROOT / "scripts/layer-spec.json").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "layer-spec.json"
            layer_spec["worker_types"]["cpu-cruise"]["artifact_family"] = "surface-cpu-production"
            path.write_text(json.dumps(layer_spec), encoding="utf-8")
            with self.assertRaises(ContractError):
                deployment_contract(SPEC_PATH, path)

    def test_cuda_header_changes_the_shared_model_source_digest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            header = write_model_source_fixture(root)
            before = model_source_recipe_sha256(root)
            header.write_text("#define FIXTURE 2\n", encoding="utf-8")
            self.assertNotEqual(before, model_source_recipe_sha256(root))

    def test_model_source_digest_ignores_checkout_ancestor_names(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            scratch = Path(temporary)
            product = scratch / "product"
            write_model_source_fixture(product)
            expected = model_source_recipe_sha256(product)
            nested = scratch / "tests/target/quietmap"
            shutil.copytree(product, nested)
            self.assertEqual(expected, model_source_recipe_sha256(nested))

    def test_model_source_digest_ignores_crate_target_directories(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_model_source_fixture(root)
            before = model_source_recipe_sha256(root)
            crate_target = root / "engine/noise-compute/target/debug"
            crate_target.mkdir(parents=True)
            (crate_target / "decoy.rs").write_text(
                "fn crate_decoy() {}\n", encoding="utf-8"
            )
            test_decoy = root / "engine/noise-compute/tests/helpers.rs"
            test_decoy.parent.mkdir()
            test_decoy.write_text("fn helper() {}\n", encoding="utf-8")
            self.assertEqual(before, model_source_recipe_sha256(root))

    def test_model_source_digest_rejects_unpruned_directory_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_model_source_fixture(root)
            elsewhere = root / "elsewhere"
            elsewhere.mkdir()
            (elsewhere / "sneaky.rs").write_text("fn sneaky() {}\n", encoding="utf-8")
            (root / "engine/noise-compute/vendor").symlink_to(elsewhere)
            with self.assertRaisesRegex(ContractError, "symlink: engine/noise-compute/vendor"):
                model_source_recipe_sha256(root)

    def test_unknown_family_field_is_rejected(self) -> None:
        self.validate_mutation(lambda spec: spec["families"]["airborne-production"].update(foo=1))

    def test_feature_unification_is_rejected(self) -> None:
        self.validate_mutation(
            lambda spec: spec["families"]["airborne-production"]["roles"][
                "airborne-stock-v1"
            ].update(cargo_features=["gpu", "unknown-feature"])
        )

    def test_airborne_role_cannot_change_its_ptx_contract(self) -> None:
        self.validate_mutation(
            lambda spec: spec["families"]["airborne-production"]["roles"][
                "airborne-stock-v1"
            ]["required_ptx_entries"].pop()
        )

    def test_role_cannot_move_between_binary_families(self) -> None:
        self.validate_mutation(
            lambda spec: spec["families"]["airborne-production"]["roles"][
                "airborne-stock-v1"
            ].update(binary="relevant-source-surface", ptx=[])
        )


class CudaFatbinTests(unittest.TestCase):
    def test_fatbin_parser_reads_images_and_refuses_damaged_executables(self) -> None:
        executable = fake_cuda_executable([75, 120], [120])
        self.assertEqual(cuda_fatbin_images(executable), (["sm_75", "sm_120"], ["sm_120"]))
        self.assertEqual(cuda_fatbin_images(fake_cuda_executable([], [])), ([], []))
        with self.assertRaisesRegex(ContractError, "extended ELF section numbering"):
            cuda_fatbin_images(executable[:0x3C] + struct.pack("<H", 0) + executable[0x3E:])
        # Every fatbin section counts: a second `.nv_fatbin` cannot hide the first.
        self.assertEqual(
            cuda_fatbin_images(fake_cuda_executable([75], [], ([120], [120]))),
            (["sm_75", "sm_120"], ["sm_120"]),
        )
        container = executable.index(struct.pack("<I", 0xBA55ED50))
        entry = container + 16
        # The `.nv_fatbin` section header is the second table row; sh_size sits at byte 32.
        (table,) = struct.unpack_from("<Q", executable, 0x28)
        size_field = table + 64 + 32
        damaged = (
            ("not ELF", b"#!/bin/sh\nsm_75,sm_120\n"),
            ("no fatbin section", executable.replace(b".nv_fatbin", b".nv_fatbix")),
            ("wrong magic", executable.replace(struct.pack("<I", 0xBA55ED50), b"\0\0\0\0", 1)),
            ("zero container header", executable[: container + 6] + b"\0\0" + executable[container + 8 :]),
            ("zero entry header", executable[: entry + 4] + b"\0" * 4 + executable[entry + 8 :]),
            ("section past the file", executable[:size_field] + b"\xff" * 8 + executable[size_field + 8 :]),
            ("section header size 32", executable[:0x3A] + struct.pack("<H", 32) + executable[0x3C:]),
            ("names not a string table", executable[: table + 128 + 4] + b"\1\0\0\0" + executable[table + 128 + 8 :]),
            ("empty image", executable[: entry + 8] + b"\0" * 8 + executable[entry + 16 :]),
            ("truncated", executable[:-40]),
        )
        for label, binary_bytes in damaged:
            with self.subTest(label=label):
                with self.assertRaises(ContractError):
                    cuda_fatbin_images(binary_bytes)


class FakeArtifactBuildTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "scripts").mkdir()
        (self.source / "engine/noise-gpu").mkdir(parents=True)
        (self.source / "engine/relevant-source-gpu").mkdir(parents=True)
        (self.source / ".cargo").mkdir()
        shutil.copyfile(SPEC_PATH, self.source / "scripts/model-role-spec.json")
        shutil.copyfile(BUILDER, self.source / "scripts/build-gpu-model-role.py")
        shutil.copyfile(
            ROOT / "scripts/gpu_model_role.py", self.source / "scripts/gpu_model_role.py"
        )
        (self.source / "engine/noise-gpu/Cargo.toml").write_text(
            '[package]\nname="noise-gpu"\nversion="0.0.0"\n', encoding="utf-8"
        )
        (self.source / "engine/relevant-source-gpu/Cargo.toml").write_text(
            '[package]\nname="relevant-source-gpu"\nversion="0.0.0"\n', encoding="utf-8"
        )
        fleet_archs = relevant_source_fleet_cuda_archs(ROOT)
        (self.source / "engine/relevant-source-gpu/relevant_source_build.rs").write_text(
            'const FLEET_CUDA_ARCHS: &[&str] = &['
            + ", ".join(f'"{arch}"' for arch in fleet_archs)
            + "];\n",
            encoding="utf-8",
        )
        (self.source / "engine/Cargo.lock").write_text(
            "# fake locked source\n", encoding="utf-8"
        )
        (self.source / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel="1.99.0"\n', encoding="utf-8"
        )
        (self.source / ".cargo/config.toml").write_text("[build]\n", encoding="utf-8")
        (self.source / "AGENTS.md").write_text("# Fake source instructions\n", encoding="utf-8")
        (self.source / "CLAUDE.md").symlink_to("AGENTS.md")
        subprocess.run(["git", "init", "-q"], cwd=self.source, check=True)
        subprocess.run(["git", "add", "."], cwd=self.source, check=True)
        subprocess.run(
            [
                "git",
                "-c",
                "user.name=role-test",
                "-c",
                "user.email=role-test@example.invalid",
                "commit",
                "-qm",
                "fake source",
            ],
            cwd=self.source,
            check=True,
        )
        self.commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.source, text=True
        ).strip()
        self.archive = self.root / "source.tar"
        with self.archive.open("wb") as output:
            subprocess.run(
                ["git", "archive", "--format=tar", self.commit],
                cwd=self.source,
                stdout=output,
                check=True,
            )

        self.tools = self.root / "tools"
        self.cuda = self.root / "cuda-13.3"
        self.tools.mkdir()
        (self.cuda / "bin").mkdir(parents=True)
        write_executable(
            self.tools / "rustc",
            "#!/bin/sh\necho 'rustc 1.99.0 (fake)'\necho 'host: x86_64-unknown-linux-gnu'\n",
        )
        write_executable(
            self.tools / "cargo",
            """#!/bin/sh
set -eu
if [ "${1:-}" = -vV ]; then echo 'cargo 1.99.0 (fake)'; echo 'host: x86_64-unknown-linux-gnu'; exit 0; fi
binary=
while [ "$#" -gt 0 ]; do
  if [ "$1" = --bin ]; then binary=$2; shift 2; else shift; fi
done
[ -n "$binary" ]
out="$CARGO_TARGET_DIR/release/build/noise-gpu-fake/out"
mkdir -p "$out" "$CARGO_TARGET_DIR/release"
if [ "$binary" = relevant-source-surface ]; then
  [ -z "$NOISE_GPU_ARCH" ]
  python3 "$(dirname "$0")/fake-cuda-executable.py" "$CARGO_TARGET_DIR/release/$binary" '__FAKE_FLEET_CUDA_ARCHS__' ''
else
  [ "$binary" = gpu-airborne ]
  ptx=airborne.ptx
  entries='.visible .entry airborne_classify_count() { ret; }
.visible .entry airborne_classify_scatter() { ret; }
.visible .entry airborne_coarse_screened() { ret; }
.visible .entry airborne_exact_screened() { ret; }
.visible .entry airborne_terrain_horizon_build() { ret; }
.visible .entry airborne_terrain_horizon_global_max() { ret; }
.visible .entry airborne_building_horizon_build() { ret; }
.visible .entry airborne_building_horizon_pack() { ret; }
.visible .entry airborne_building_horizon_global_max() { ret; }
.visible .entry airborne_building_horizon_mark_empty() { ret; }'
fi
if [ "$binary" != relevant-source-surface ]; then
  printf '#!/bin/sh\nembedded PTX follows\n' > "$CARGO_TARGET_DIR/release/$binary"
  printf '.version 8.8\n.target sm_120\n.address_size 64\n%s\n' "$entries" > "$out/$ptx"
  cat "$out/$ptx" >> "$CARGO_TARGET_DIR/release/$binary"
fi
chmod +x "$CARGO_TARGET_DIR/release/$binary"
printf '%s\n' '-DTPX=512' > "$out/nvcc-defines.txt"
""",
        )
        fake_cargo = self.tools / "cargo"
        fake_cargo.write_text(
            fake_cargo.read_text(encoding="utf-8").replace(
                "__FAKE_FLEET_CUDA_ARCHS__",
                ",".join(relevant_source_fleet_cuda_archs(self.source)),
            ),
            encoding="utf-8",
        )
        fake_cargo.chmod(0o755)
        (self.tools / "fake-cuda-executable.py").write_text(
            FAKE_CUDA_EXECUTABLE_SOURCE, encoding="utf-8"
        )
        write_executable(
            self.cuda / "bin/nvcc",
            "#!/bin/sh\necho 'Cuda compilation tools, release 13.3, V13.3.0'\n",
        )
        write_executable(
            self.cuda / "bin/ptxas",
            """#!/bin/sh
set -eu
if [ "${1:-}" = --version ]; then echo 'ptxas release 13.3, V13.3.0'; exit 0; fi
out=
while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then out=$2; shift 2; else shift; fi
done
[ -n "$out" ]
: > "$out"
echo 'ptxas info : 0 bytes stack frame, 0 bytes spill stores, 0 bytes spill loads' >&2
echo 'ptxas info : Function properties for airborne kernels airborne_classify_count airborne_classify_scatter airborne_coarse_screened airborne_exact_screened airborne_terrain_horizon_build airborne_terrain_horizon_global_max airborne_building_horizon_build airborne_building_horizon_pack airborne_building_horizon_global_max airborne_building_horizon_mark_empty' >&2
""",
        )
        self.artifacts = self.root / "artifacts"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def command(
        self,
        role: str = "airborne-stock-v1",
        family: str = "airborne-production",
        arch: str | None = "sm_120",
    ) -> list[str]:
        command = [
            sys.executable,
            str(BUILDER),
            "--source-archive",
            str(self.archive),
            "--source-archive-sha256",
            sha256(self.archive),
            "--product-commit",
            self.commit,
            "--role-spec-sha256",
            sha256(SPEC_PATH),
            "--builder-sha256",
            sha256(BUILDER),
            "--contract-sha256",
            sha256(ROOT / "scripts/gpu_model_role.py"),
            "--family",
            family,
            "--role",
            role,
            "--artifact-root",
            str(self.artifacts),
            "--cuda-root",
            str(self.cuda),
        ]
        if arch is not None:
            command.extend(["--arch", arch])
        return command

    def run_builder(self, command: list[str], **environment) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(environment)
        env["PATH"] = f"{self.tools}:{env['PATH']}"
        return subprocess.run(command, text=True, capture_output=True, env=env, check=False)

    def replace_archive_argument(self, command: list[str], archive: Path) -> list[str]:
        changed = list(command)
        archive_index = changed.index("--source-archive") + 1
        archive_sha_index = changed.index("--source-archive-sha256") + 1
        changed[archive_index] = str(archive)
        changed[archive_sha_index] = sha256(archive)
        return changed

    def commit_and_archive_source(self, message: str) -> None:
        subprocess.run(["git", "add", "-A"], cwd=self.source, check=True)
        subprocess.run(
            [
                "git",
                "-c",
                "user.name=role-test",
                "-c",
                "user.email=role-test@example.invalid",
                "commit",
                "-qm",
                message,
            ],
            cwd=self.source,
            check=True,
        )
        self.commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.source, text=True
        ).strip()
        with self.archive.open("wb") as output:
            subprocess.run(
                ["git", "archive", "--format=tar", self.commit],
                cwd=self.source,
                stdout=output,
                check=True,
            )

    def test_fake_toolchain_builds_and_replays_one_immutable_role(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        receipt = verify_artifact(artifact, SPEC_PATH)
        self.assertEqual(receipt["build"]["cuda_context"], "not_opened")
        self.assertIn(
            "airborne_classify_scatter", receipt["ptx"]["airborne.ptx"]["entries"]
        )
        self.assertTrue((artifact / "BUILD_TERMINAL").is_file())
        self.assertFalse((artifact / "receipts/feature-receipt.txt").exists())
        source_manifest = (artifact / "input/source-files.sha256").read_text(encoding="utf-8")
        symlink_identity = hashlib.sha256(
            b"quietmap-source-symlink-v1\0AGENTS.md"
        ).hexdigest()
        self.assertIn(f"{symlink_identity}  CLAUDE.md\n", source_manifest)
        self.assertNotEqual(symlink_identity, sha256(self.source / "AGENTS.md"))

        byte_different_spec = self.root / "byte-different-role-spec.json"
        byte_different_spec.write_text(
            json.dumps(json.loads(SPEC_PATH.read_text(encoding="utf-8"))),
            encoding="utf-8",
        )
        with self.assertRaises(ContractError):
            verify_artifact(artifact, byte_different_spec)

        repeated = self.run_builder(self.command())
        self.assertNotEqual(repeated.returncode, 0)
        self.assertIn("already exists", repeated.stderr)

    def test_fleet_role_builds_all_images_without_a_single_arch_pin(self) -> None:
        role = "relevant-source-stock-v1"
        result = self.run_builder(self.command(role, "relevant-source-production", None))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / role
        receipt = verify_artifact(artifact, SPEC_PATH)
        self.assertIsNone(receipt["build"]["arch"])
        self.assertEqual(receipt["build"]["environment"]["NOISE_GPU_ARCH"], "")
        fleet_archs = relevant_source_fleet_cuda_archs(ROOT)
        binary = artifact / "relevant-source-surface"
        self.assertEqual(cuda_fatbin_images(binary.read_bytes()), (fleet_archs, []))
        self.assertFalse((artifact / "receipts/cuda-images.txt").exists())

    def test_fleet_role_rejects_a_single_arch_pin_and_ptx_roles_require_one(self) -> None:
        fleet = self.run_builder(
            self.command("relevant-source-stock-v1", "relevant-source-production")
        )
        self.assertNotEqual(fleet.returncode, 0)
        self.assertIn("fleet CUDA image roles do not accept --arch", fleet.stderr)
        ptx = self.run_builder(self.command(arch=None))
        self.assertNotEqual(ptx.returncode, 0)
        self.assertIn("single-arch GPU roles require --arch", ptx.stderr)

    def test_artifact_set_uses_the_selected_relevant_source_role(self) -> None:
        result = self.run_builder(
            self.command("relevant-source-stock-v1", "relevant-source-production", None)
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        layer_spec = ROOT / "scripts/layer-spec.json"
        identity = artifact_set(
            SPEC_PATH, layer_spec, self.artifacts, ["relevant-source-production"]
        )
        artifact = identity["artifacts"]["relevant-source-production"]
        self.assertEqual(artifact["resolved_role"], "relevant-source-stock-v1")
        self.assertEqual(artifact["binary"], "relevant-source-surface")
        expected_contract = deployment_contract(SPEC_PATH, layer_spec)
        self.assertEqual(
            identity["line_model_role_sha256"], expected_contract["line_model_role_sha256"]
        )
        cli = subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts/gpu_model_role.py"),
                "artifact-set",
                str(SPEC_PATH),
                str(layer_spec),
                str(self.artifacts),
                "relevant-source-production",
            ],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(cli.returncode, 0, cli.stdout + cli.stderr)
        self.assertEqual(json.loads(cli.stdout), identity)

    def test_airborne_role_builds_only_its_declared_binary_and_ptx(self) -> None:
        result = self.run_builder(
            self.command("airborne-stock-v1", "airborne-production")
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        receipt = verify_artifact(artifact, SPEC_PATH)
        self.assertEqual(receipt["binary"], "gpu-airborne")
        self.assertEqual(set(receipt["ptx"]), {"airborne.ptx"})

    def test_archive_hash_and_unknown_role_fail_before_artifact_creation(self) -> None:
        bad_hash = self.command()
        index = bad_hash.index("--source-archive-sha256") + 1
        bad_hash[index] = "0" * 64
        result = self.run_builder(bad_hash)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.artifacts.exists())

        unknown = self.command("airborne-invalid-e1")
        result = self.run_builder(unknown)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.artifacts / "airborne-invalid-e1").exists())

    def test_archive_rejects_unsafe_or_noncanonical_symlink_targets(self) -> None:
        for sequence, linkname in enumerate(
            ["/etc/passwd", "../AGENTS.md", "./AGENTS.md", "nested//AGENTS.md"]
        ):
            with self.subTest(linkname=linkname):
                (self.source / "CLAUDE.md").unlink()
                (self.source / "CLAUDE.md").symlink_to(linkname)
                self.commit_and_archive_source(f"unsafe symlink {sequence}")
                result = self.run_builder(self.command())
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unsafe or non-canonical target", result.stderr)

    def test_archive_rejects_hardlinks_and_special_entries(self) -> None:
        for sequence, entry_type in enumerate((tarfile.LNKTYPE, tarfile.FIFOTYPE)):
            with self.subTest(entry_type=entry_type):
                archive = self.root / f"special-{sequence}.tar"
                shutil.copyfile(self.archive, archive)
                with tarfile.open(archive, mode="a:") as target:
                    member = tarfile.TarInfo(f"special-{sequence}")
                    member.type = entry_type
                    if entry_type == tarfile.LNKTYPE:
                        member.linkname = "AGENTS.md"
                    target.addfile(member)
                result = self.run_builder(
                    self.replace_archive_argument(self.command(), archive)
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("source archive contains non-file entry", result.stderr)

    def test_tampered_payload_is_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        with (artifact / "gpu-airborne").open("ab") as output:
            output.write(b"tampered")
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)


class FakeRustArtifactBuildTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        for path in (
            "scripts",
            "engine/tile-painter/src",
            "engine/noise-compute/src",
            "engine/noise-gpu/src",
            "engine/source-reader/src",
            ".cargo",
        ):
            (self.source / path).mkdir(parents=True, exist_ok=True)
        for path in (
            "scripts/model-role-spec.json",
            "scripts/build-rust-model-role.py",
            "scripts/gpu_model_role.py",
        ):
            shutil.copyfile(ROOT / path, self.source / path)
        for crate in ("tile-painter", "noise-compute", "noise-gpu", "source-reader"):
            (self.source / f"engine/{crate}/Cargo.toml").write_text(
                f'[package]\nname="{crate}"\nversion="0.0.0"\n', encoding="utf-8"
            )
            (self.source / f"engine/{crate}/src/lib.rs").write_text(
                "//! fake source\n", encoding="utf-8"
            )
        (self.source / "engine/Cargo.lock").write_text(
            "# fake locked source\n", encoding="utf-8"
        )
        (self.source / "engine/tile-painter/src/wire_hm3.rs").write_text(
            "pub const VERSION: u8 = 3;\n", encoding="utf-8"
        )
        (self.source / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel="1.99.0"\n', encoding="utf-8"
        )
        (self.source / ".cargo/config.toml").write_text("[build]\n", encoding="utf-8")
        (self.source / "AGENTS.md").write_text("# Fake source instructions\n", encoding="utf-8")
        (self.source / "CLAUDE.md").symlink_to("AGENTS.md")
        subprocess.run(["git", "init", "-q"], cwd=self.source, check=True)
        subprocess.run(["git", "add", "."], cwd=self.source, check=True)
        subprocess.run(
            ["git", "-c", "user.name=role-test", "-c", "user.email=role-test@example.invalid",
             "commit", "-qm", "fake source"],
            cwd=self.source,
            check=True,
        )
        self.commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.source, text=True
        ).strip()
        self.archive = self.root / "source.tar"
        with self.archive.open("wb") as output:
            subprocess.run(
                ["git", "archive", "--format=tar", self.commit],
                cwd=self.source,
                stdout=output,
                check=True,
            )
        self.tools = self.root / "tools"
        self.tools.mkdir()
        write_executable(
            self.tools / "rustc",
            "#!/bin/sh\necho 'rustc 1.99.0 (fake)'\necho 'host: x86_64-unknown-linux-gnu'\n",
        )
        write_executable(
            self.tools / "cargo",
            """#!/bin/sh
set -eu
if [ "${1:-}" = -vV ]; then echo 'cargo 1.99.0 (fake)'; exit 0; fi
binary=
while [ "$#" -gt 0 ]; do
  if [ "$1" = --bin ]; then binary=$2; shift 2; else shift; fi
done
[ -n "$binary" ]
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/bin/sh\nexit 0\n' > "$CARGO_TARGET_DIR/release/$binary"
chmod +x "$CARGO_TARGET_DIR/release/$binary"
""",
        )
        self.artifacts = self.root / "artifacts"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def command(self) -> list[str]:
        return [
            sys.executable, str(RUST_BUILDER),
            "--source-archive", str(self.archive),
            "--source-archive-sha256", sha256(self.archive),
            "--product-commit", self.commit,
            "--role-spec-sha256", sha256(SPEC_PATH),
            "--builder-sha256", sha256(RUST_BUILDER),
            "--contract-sha256", sha256(ROOT / "scripts/gpu_model_role.py"),
            "--family", "surface-cpu-production",
            "--role", "surface-cpu-stock-v1",
            "--artifact-root", str(self.artifacts),
        ]

    def run_builder(self) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment["PATH"] = f"{self.tools}:{environment['PATH']}"
        return subprocess.run(
            self.command(), text=True, capture_output=True, env=environment, check=False
        )

    def test_fake_cpu_role_builds_replays_and_is_immutable(self) -> None:
        result = self.run_builder()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-cpu-stock-v1"
        receipt = verify_rust_artifact(artifact, SPEC_PATH)
        self.assertEqual(receipt["binary"], "build-heatmap-surface")
        self.assertEqual(receipt["cargo_features"], [])
        self.assertTrue((artifact / "build-heatmap-surface").is_file())
        source_manifest = (artifact / "input/source-files.sha256").read_text(encoding="utf-8")
        symlink_identity = hashlib.sha256(
            b"quietmap-source-symlink-v1\0AGENTS.md"
        ).hexdigest()
        self.assertIn(f"{symlink_identity}  CLAUDE.md\n", source_manifest)
        self.assertNotEqual(symlink_identity, sha256(self.source / "AGENTS.md"))
        repeated = self.run_builder()
        self.assertNotEqual(repeated.returncode, 0)

    def test_tampered_cpu_role_payload_is_rejected(self) -> None:
        result = self.run_builder()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-cpu-stock-v1"
        with (artifact / "build-heatmap-surface").open("ab") as output:
            output.write(b"tampered")
        with self.assertRaises(ContractError):
            verify_rust_artifact(artifact, SPEC_PATH)


class FakeArtifactMutationTests(FakeArtifactBuildTests):
    """Resealed attacks exercise the GPU artifact verifier after a valid fake build."""

    def test_resealed_semantic_role_mutation_is_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        receipt_path = artifact / "artifact-receipt.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["selected"] = False
        receipt_path.write_text(
            json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

    def test_resealed_embedded_ptx_mutation_is_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        binary = artifact / "gpu-airborne"
        binary.write_bytes(
            binary.read_bytes().replace(
                b"airborne_classify_scatter", b"airborne_classify_mutant"
            )
        )
        binary.chmod(0o755)
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

    def test_resealed_arch_must_match_the_ptx_target(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        receipt_path = artifact / "artifact-receipt.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["build"]["arch"] = "sm_89"
        receipt["build"]["environment"]["NOISE_GPU_ARCH"] = "sm_89"
        receipt_path.write_text(
            json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)


if __name__ == "__main__":
    unittest.main(verbosity=2)
