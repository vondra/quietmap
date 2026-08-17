#!/usr/bin/env python3
"""Mutation and fake-toolchain tests for GPU model-role artifacts."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from gpu_model_role import (
    EXPERIMENTAL_DEFINE_NAMES,
    ContractError,
    load_and_validate_spec,
    resolve_role,
    verify_artifact,
)


ROOT = Path(__file__).resolve().parent.parent
SPEC_PATH = ROOT / "scripts/model-role-spec.json"
BUILDER = ROOT / "scripts/build-gpu-model-role.py"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_executable(path: Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(0o755)


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

    def test_checked_in_spec_is_stock_only_and_resolves_exact_roles(self) -> None:
        spec = load_and_validate_spec(SPEC_PATH)
        surface = resolve_role(spec, "surface-production", "surface-stock-v1")
        airborne = resolve_role(spec, "airborne-production", "airborne-stock-v1")
        self.assertTrue(surface["selected"])
        self.assertTrue(airborne["selected"])
        self.assertEqual(surface["cargo_features"], ["gpu"])
        self.assertEqual(airborne["cargo_features"], ["gpu"])
        self.assertFalse(
            any(
                role["model_role"] == "h0"
                for family in spec["families"].values()
                for role in family["roles"].values()
            ),
            "an H0 role requires the still-pending numerical selection record",
        )

    def test_unknown_family_field_is_rejected(self) -> None:
        self.validate_mutation(lambda spec: spec["families"]["surface-production"].update(foo=1))

    def test_feature_unification_is_rejected(self) -> None:
        self.validate_mutation(
            lambda spec: spec["families"]["surface-production"]["roles"][
                "surface-stock-v1"
            ].update(cargo_features=["gpu", "v2-h0"])
        )

    def test_h0_role_without_selected_epoch_is_rejected(self) -> None:
        def mutate(spec) -> None:
            role = spec["families"]["surface-production"]["roles"].pop("surface-stock-v1")
            role.update(model_role="h0", cargo_features=["v2-h0"])
            spec["families"]["surface-production"]["roles"] = {"surface-h0-e0": role}
            spec["families"]["surface-production"]["selected_role"] = "surface-h0-e0"

        self.validate_mutation(mutate)

    def test_role_cannot_move_between_binary_families(self) -> None:
        self.validate_mutation(
            lambda spec: spec["families"]["surface-production"]["roles"][
                "surface-stock-v1"
            ].update(binary="gpu-airborne", ptx=["airborne.ptx"])
        )

    def test_artifact_verifier_allowlist_matches_the_rust_parser(self) -> None:
        source = (ROOT / "engine/noise-gpu/build_defines.rs").read_text(encoding="utf-8")
        matched = re.search(
            r"const REVIEWED_EXPERIMENTAL_DEFINES: &\[&str\] = &\[(.*?)\];",
            source,
            flags=re.DOTALL,
        )
        self.assertIsNotNone(matched)
        rust_names = set(re.findall(r'"([A-Z][A-Z0-9_]*)"', matched.group(1)))
        self.assertEqual(rust_names, EXPERIMENTAL_DEFINE_NAMES)


class FakeArtifactBuildTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "scripts").mkdir()
        (self.source / "engine/noise-gpu").mkdir(parents=True)
        (self.source / ".cargo").mkdir()
        shutil.copyfile(SPEC_PATH, self.source / "scripts/model-role-spec.json")
        shutil.copyfile(BUILDER, self.source / "scripts/build-gpu-model-role.py")
        shutil.copyfile(
            ROOT / "scripts/gpu_model_role.py", self.source / "scripts/gpu_model_role.py"
        )
        (self.source / "engine/noise-gpu/Cargo.toml").write_text(
            '[package]\nname="noise-gpu"\nversion="0.0.0"\n', encoding="utf-8"
        )
        (self.source / "engine/noise-gpu/Cargo.lock").write_text(
            "# fake locked source\n", encoding="utf-8"
        )
        (self.source / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel="1.99.0"\n', encoding="utf-8"
        )
        (self.source / ".cargo/config.toml").write_text("[build]\n", encoding="utf-8")
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
if [ "$binary" = gpu-surface ]; then
  ptx=scatter.ptx
  entries='.visible .entry line_binned_fused() { ret; }'
else
  ptx=airborne.ptx
  entries='.visible .entry airborne_classify_count() { ret; }
.visible .entry airborne_classify_scatter() { ret; }
.visible .entry airborne_coarse_batched() { ret; }
.visible .entry airborne_exact_batched() { ret; }'
fi
printf '.version 8.8\n.target sm_120\n.address_size 64\n%s\n' "$entries" > "$out/$ptx"
printf '#!/bin/sh\nembedded PTX follows\n' > "$CARGO_TARGET_DIR/release/$binary"
cat "$out/$ptx" >> "$CARGO_TARGET_DIR/release/$binary"
chmod +x "$CARGO_TARGET_DIR/release/$binary"
printf '#define V2_H0 0\n#define BARRIER_ABI_VERSION 2\n' > "$out/qm_streaming_abi_generated.h"
printf '%s\n' '-DV2_H0=0' '-DBARRIER_ABI_VERSION=2' > "$out/nvcc-defines.txt"
""",
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
echo 'ptxas info : Function properties for line_binned_fused airborne_classify_count airborne_classify_scatter airborne_coarse_batched airborne_exact_batched' >&2
""",
        )
        self.artifacts = self.root / "artifacts"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def command(
        self,
        role: str = "surface-stock-v1",
        family: str = "surface-production",
    ) -> list[str]:
        return [
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
            "--arch",
            "sm_120",
        ]

    def run_builder(self, command: list[str], **environment) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(environment)
        env["PATH"] = f"{self.tools}:{env['PATH']}"
        return subprocess.run(command, text=True, capture_output=True, env=env, check=False)

    def test_fake_toolchain_builds_and_replays_one_immutable_role(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
        receipt = verify_artifact(artifact, SPEC_PATH)
        self.assertEqual(receipt["build"]["cuda_context"], "not_opened")
        self.assertEqual(receipt["build"]["environment"]["NOISE_GPU_DEFINES"], "")
        self.assertIn("line_binned_fused", receipt["ptx"]["scatter.ptx"]["entries"])
        self.assertTrue((artifact / "BUILD_TERMINAL").is_file())
        self.assertFalse((artifact / "receipts/feature-receipt.txt").exists())

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

    def test_airborne_role_builds_only_its_declared_binary_and_ptx(self) -> None:
        result = self.run_builder(
            self.command("airborne-stock-v1", "airborne-production")
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "airborne-stock-v1"
        receipt = verify_artifact(artifact, SPEC_PATH)
        self.assertEqual(receipt["binary"], "gpu-airborne")
        self.assertEqual(set(receipt["ptx"]), {"airborne.ptx"})
        self.assertFalse((artifact / "gpu-surface").exists())
        self.assertFalse((artifact / "ptx/scatter.ptx").exists())

    def test_archive_hash_and_unknown_role_fail_before_artifact_creation(self) -> None:
        bad_hash = self.command()
        index = bad_hash.index("--source-archive-sha256") + 1
        bad_hash[index] = "0" * 64
        result = self.run_builder(bad_hash)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.artifacts.exists())

        unknown = self.command("surface-h0-e1")
        result = self.run_builder(unknown)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.artifacts / "surface-h0-e1").exists())

    def test_nonempty_caller_define_is_rejected(self) -> None:
        result = self.run_builder(self.command(), NOISE_GPU_DEFINES="-DV2_H0=1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("caller NOISE_GPU_DEFINES must be empty", result.stderr)

    def test_tampered_payload_is_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
        with (artifact / "gpu-surface").open("ab") as output:
            output.write(b"tampered")
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

    def test_resealed_semantic_role_mutation_is_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
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

    def test_resealed_define_and_embedded_ptx_mutations_are_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
        define_receipt = artifact / "receipts/nvcc-defines.txt"
        define_receipt.write_text(
            define_receipt.read_text(encoding="utf-8").replace("-DV2_H0=0", "-DV2_H0=1"),
            encoding="utf-8",
        )
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

        define_receipt.write_text(
            define_receipt.read_text(encoding="utf-8").replace("-DV2_H0=1", "-DV2_H0=0"),
            encoding="utf-8",
        )
        binary = artifact / "gpu-surface"
        binary.write_bytes(
            binary.read_bytes().replace(b"line_binned_fused", b"line_binned_mutant")
        )
        binary.chmod(0o755)
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

    def test_resealed_experimental_define_is_rejected_even_if_environment_agrees(
        self,
    ) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
        define_receipt = artifact / "receipts/nvcc-defines.txt"
        with define_receipt.open("a", encoding="utf-8") as output:
            output.write("-DPROF_COUNTERS=1\n")
        receipt_path = artifact / "artifact-receipt.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["build"]["environment"]["NOISE_GPU_DEFINES"] = "-DPROF_COUNTERS=1"
        receipt_path.write_text(
            json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

    def test_resealed_environment_define_without_receipt_is_rejected(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
        receipt_path = artifact / "artifact-receipt.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["build"]["environment"]["NOISE_GPU_DEFINES"] = "-DPROF_COUNTERS=1"
        receipt_path.write_text(
            json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        reseal_artifact(artifact)
        with self.assertRaises(ContractError):
            verify_artifact(artifact, SPEC_PATH)

    def test_resealed_arch_must_match_the_ptx_target(self) -> None:
        result = self.run_builder(self.command())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        artifact = self.artifacts / "surface-stock-v1"
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
