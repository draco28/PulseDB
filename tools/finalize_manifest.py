#!/usr/bin/env python3
"""Layer mechanical, environment provenance onto a fixture CONTENT manifest.

The Rust generator emits a `*.content.json` (ground-truth values + genuine
on-disk copy-through raw bytes). This step adds the parts that can only be known
AFTER the blob is frozen or that live outside the process (audit C1):

  - blob_sha256          SHA-256 of the frozen .redb (the AUTO provenance AC in
                         4.01 recomputes + compares this)
  - blob_size_bytes
  - generator_git_commit the commit the generator was built from
  - generator_cargo_lock_sha256
  - resolved_dependency_checksums  resolved crates.io checksums (name+version+
                         checksum) parsed from the generator's Cargo.lock
  - build_env.{rustc,cargo,toolchain}  merged onto the generator-supplied os/arch

Usage:
  finalize_manifest.py <content.json> <blob.redb> <Cargo.lock> <out.manifest.json> \
                       <git_commit> <rustc_version> <cargo_version>
"""
import hashlib
import json
import re
import sys


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_cargo_lock(path):
    """Extract resolved (name, version, checksum) for every registry package.

    Deliberately dependency-free (no tomllib requirement): Cargo.lock is a flat
    sequence of `[[package]]` blocks with simple `key = "value"` lines.
    """
    text = open(path, "r", encoding="utf-8").read()
    deps = []
    for block in text.split("[[package]]")[1:]:
        # stop at the next top-level table if any snuck in
        block = block.split("\n[", 1)[0]
        name = re.search(r'^\s*name\s*=\s*"([^"]+)"', block, re.M)
        version = re.search(r'^\s*version\s*=\s*"([^"]+)"', block, re.M)
        checksum = re.search(r'^\s*checksum\s*=\s*"([0-9a-fA-F]+)"', block, re.M)
        if name and version and checksum:
            deps.append(
                {
                    "name": name.group(1),
                    "version": version.group(1),
                    "checksum": checksum.group(1),
                }
            )
    deps.sort(key=lambda d: (d["name"], d["version"]))
    return deps


def main():
    (
        content_path,
        blob_path,
        lock_path,
        out_path,
        git_commit,
        rustc_version,
        cargo_version,
    ) = sys.argv[1:8]

    manifest = json.load(open(content_path, "r", encoding="utf-8"))

    manifest["blob_sha256"] = sha256_file(blob_path)
    manifest["blob_size_bytes"] = __import__("os").path.getsize(blob_path)
    manifest["generator_git_commit"] = git_commit
    manifest["generator_cargo_lock_sha256"] = sha256_file(lock_path)
    manifest["resolved_dependency_checksums"] = parse_cargo_lock(lock_path)

    build_env = manifest.get("build_env", {})
    build_env["rustc"] = rustc_version
    build_env["cargo"] = cargo_version
    build_env["toolchain"] = rustc_version
    manifest["build_env"] = build_env

    manifest["provenance_note"] = (
        "Mechanical provenance (audit C1): blob_sha256 is recomputed + compared by "
        "4.01's AUTO provenance AC; a mutated/truncated blob fails that check loudly. "
        "generator_git_commit + generator_cargo_lock_sha256 + resolved_dependency_checksums "
        "+ build_env pin the exact generation environment. The generator is committed for "
        "provenance but is NOT run in CI (4.02 runs the upgrade test against the FROZEN blob)."
    )

    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write("\n")

    print(
        f"finalize: {out_path} blob_sha256={manifest['blob_sha256'][:16]}... "
        f"deps={len(manifest['resolved_dependency_checksums'])} "
        f"size={manifest['blob_size_bytes']}"
    )


if __name__ == "__main__":
    main()
