#!/usr/bin/env python3
"""Wiring gate: the initrd manifest must carry every binary ade can launch.

Class of bug caught: ade's app catalog exec'd /bin/skyedit, /bin/skyfiles
and /bin/sargaview while the initrd shipped none of them (sargaedit was
renamed to /bin/sargaedit, sargafiles/sargaview not shipped at all) -- so
SkyEdit, File Manager and Image Viewer could not launch on the real OS.
build_initrd.py only WARNS on a missing binary, so the image silently
shipped without them.

Three legs:
  1. Every non-placeholder catalog exec resolves to a shipped bin name.
  2. Every shipped manifest name is an actual built binary (cargo metadata).
  3. The placeholder allowlist never covers a shipped binary (so a shipped
     app leaves the allowlist, and removing it later is caught again).
"""
import json
import os
import re
import subprocess
import sys
import unittest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, REPO_ROOT)
import build_initrd  # noqa: E402 -- real manifest, must come after sys.path

# Apps in ade's catalog with no crate yet (aspirational rows). Launching
# them shows the row but fails to exec; shipping nothing keeps that honest.
PLACEHOLDER_EXECS = {
    "/bin/skywebbrowser",
    "/bin/skymail",
    "/bin/skymedia",
    "/bin/skyrecorder",
    "/bin/skychess",
    "/bin/skysudoku",
    "/bin/skyextractor",
    "/bin/skydisk",
    "/bin/skyscreenshot",
}


def shipped_names():
    # Initrd FILE basenames (manifest KEYS): the names exec paths
    # resolve against on the real OS (e.g. /bin/skyedit ships the
    # sargaedit binary under the skyedit name). The manifest
    # VALUES are the built binaries behind each file -- those are
    # checked against cargo metadata in
    # test_every_shipped_name_is_a_built_binary.
    names = set(build_initrd.COREUTILS_BINS)
    names.update(name.rsplit("/", 1)[-1] for name in build_initrd.BINARIES)
    return names


def catalog_execs():
    with open(
        os.path.join(REPO_ROOT, "ade", "src", "util", "app_catalog.rs"),
        encoding="utf-8",
    ) as fh:
        src = fh.read()
    return re.findall(r'exec: "([^"]*)"', src)


def built_bins():
    r = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        raise AssertionError("cargo metadata failed: " + r.stderr[-500:])
    meta = json.loads(r.stdout)
    out = set()
    for pkg in meta["packages"]:
        for t in pkg["targets"]:
            if t["kind"] and t["kind"][0] == "bin":
                out.add(t["name"])
    return out


class TestAppCatalogWiring(unittest.TestCase):
    def test_catalog_execs_resolve_to_shipped_bins(self):
        shipped = shipped_names()
        broken = []
        for ex in catalog_execs():
            if not ex or ex in PLACEHOLDER_EXECS:
                continue
            name = ex.rsplit("/", 1)[-1]
            if name not in shipped:
                broken.append(ex)
        self.assertEqual(
            broken, [],
            "app catalog execs not present in the initrd manifest: %s" % broken,
        )

    def test_every_shipped_name_is_a_built_binary(self):
        # Manifest VALUES are the actual built binaries (the
        # sargaedit binary behind /bin/skyedit); every one must
        # exist in cargo metadata or the image silently ships
        # without it (build_initrd.py only WARNS).
        shipped = set(build_initrd.COREUTILS_BINS) | set(build_initrd.BINARIES.values())
        missing = sorted(shipped - built_bins())
        self.assertEqual(
            missing, [],
            "initrd manifest entries with no built binary (build_initrd.py "
            "only WARNS, silently producing an image missing them): %s" % missing,
        )

    def test_placeholder_allowlist_never_covers_a_shipped_binary(self):
        shipped = shipped_names()
        stale = sorted(
            ex.rsplit("/", 1)[-1]
            for ex in PLACEHOLDER_EXECS
            if ex.rsplit("/", 1)[-1] in shipped
        )
        self.assertEqual(
            stale, [],
            "placeholder allowlist entries that are now shipped (remove them "
            "from PLACEHOLDER_EXECS so test_catalog_execs_resolve_to_shipped_"
            "bins guards them): %s" % stale,
        )


if __name__ == "__main__":
    unittest.main()
