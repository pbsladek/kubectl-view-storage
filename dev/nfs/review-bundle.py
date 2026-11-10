#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["PyYAML==6.0.3"]
# ///
"""Review-time conversion of the exact released NFS CSI manifests to pinned JSON."""

from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import subprocess
import urllib.request
import yaml

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "dev/nfs"
SOURCE = ROOT / ".local/upstream/csi-driver-nfs"
REVISION = "ef097104fe8d06ee3f8a81cb04be39f6eba0ff6f"
RELEASE = "v4.13.6"
GANESHA = "quay.io/piraeusdatastore/piraeus-csi-nfs-server:v1.13.0"
RECIPE_REVISION = "dc0fe692f42b88fd2f32beac66565fc8cfb1b6e3"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def command(*args):
    return subprocess.check_output(args, text=True, timeout=120)


def resolve(reference):
    text = command("docker", "buildx", "imagetools", "inspect", reference)
    digest = re.search(r"^Digest:\s+(sha256:[0-9a-f]{64})\s*$", text, re.M).group(1)
    image = reference + "@" + digest
    index = json.loads(
        command("docker", "buildx", "imagetools", "inspect", "--raw", image)
    )
    platforms = {}
    for arch in ("amd64", "arm64"):
        found = [
            x["digest"]
            for x in index["manifests"]
            if x["platform"].get("os") == "linux"
            and x["platform"].get("architecture") == arch
        ]
        if len(found) != 1:
            raise RuntimeError(
                "missing or ambiguous architecture: " + image + " " + arch
            )
        platforms[arch] = found[0]
    print("VERIFIED " + image, flush=True)
    return {"image": image, "platform_digests": platforms}


def containers(value):
    if isinstance(value, dict):
        for key, child in value.items():
            if key in ("containers", "initContainers"):
                yield from child
            yield from containers(child)
    elif isinstance(value, list):
        for child in value:
            yield from containers(child)


def main():
    if (
        command("git", "-C", str(SOURCE), "rev-parse", "HEAD").strip() != REVISION
        or command("git", "-C", str(SOURCE), "status", "--porcelain").strip()
    ):
        raise RuntimeError("NFS source must be clean at the reviewed release commit")
    files = [
        "rbac-csi-nfs.yaml",
        "csi-nfs-driverinfo.yaml",
        "csi-nfs-controller.yaml",
        "csi-nfs-node.yaml",
    ]
    provenance = []
    items = []
    for name in files:
        path = SOURCE / "deploy" / RELEASE / name
        provenance.append(
            {"file": str(path.relative_to(SOURCE)), "sha256": sha(path.read_bytes())}
        )
        objects = [x for x in yaml.safe_load_all(path.read_text()) if x]
        for obj in objects:
            if obj["kind"] == "Deployment":
                # Snapshots and resize are outside this storage I/O fixture.
                spec = obj["spec"]["template"]["spec"]
                spec["containers"] = [
                    c
                    for c in spec["containers"]
                    if c["name"] not in ("csi-snapshotter", "csi-resizer")
                ]
                for c in spec["containers"]:
                    if c["name"] == "csi-provisioner":
                        c["args"] = [
                            a if not a.startswith("--timeout=") else "--timeout=60s"
                            for a in c["args"]
                        ]
        items += objects
    workload = json.loads((ROOT / "dev/csi/lock.json").read_text())["workload_image"]
    references = {c["image"] for c in containers(items)} | {GANESHA}
    with ThreadPoolExecutor(max_workers=4) as pool:
        images = list(pool.map(resolve, sorted(references)))
    images.append(
        next(
            i
            for i in json.loads((ROOT / "dev/csi/lock.json").read_text())["images"]
            if i["image"] == workload
        )
    )
    pinned = {i["image"].split("@")[0]: i["image"] for i in images}
    for c in containers(items):
        c["image"] = pinned[c["image"]]
    target = DEST / "vendor/driver.json"
    target.write_text(
        json.dumps({"apiVersion": "v1", "kind": "List", "items": items}, indent=2)
        + "\n"
    )
    license = DEST / "licenses/csi-driver-nfs.txt"
    license.write_bytes((SOURCE / "LICENSE").read_bytes())
    recipe_url = f"https://raw.githubusercontent.com/piraeusdatastore/linstor-csi/{RECIPE_REVISION}/nfs/Dockerfile"
    with urllib.request.urlopen(recipe_url, timeout=30) as response:
        recipe = response.read()
    (DEST / "ganesha.Dockerfile.reference").write_bytes(recipe)
    server_license_url = f"https://raw.githubusercontent.com/piraeusdatastore/linstor-csi/{RECIPE_REVISION}/LICENSE"
    with urllib.request.urlopen(server_license_url, timeout=30) as response:
        server_license = response.read()
    (DEST / "licenses/server-recipe.txt").write_bytes(server_license)
    lock = {
        "status": "reviewed",
        "driver_release": RELEASE,
        "source_revision": REVISION,
        "sources": {
            "csi-driver-nfs": {
                "repository": "https://github.com/kubernetes-csi/csi-driver-nfs",
                "revision": REVISION,
                "files": provenance,
            }
        },
        "driver_image": pinned["registry.k8s.io/sig-storage/nfsplugin:" + RELEASE],
        "server_image": pinned[GANESHA],
        "workload_image": workload,
        "server_recipe_reference": {
            "url": recipe_url,
            "revision": RECIPE_REVISION,
            "file": "ganesha.Dockerfile.reference",
            "sha256": sha(recipe),
            "scope": "reviewed build recipe; not a verified image build attestation",
        },
        "images": images,
        "manifests": [
            {"file": "vendor/driver.json", "sha256": sha(target.read_bytes())}
        ],
        "licenses": [
            {
                "file": "licenses/csi-driver-nfs.txt",
                "sha256": sha(license.read_bytes()),
            },
            {"file": "licenses/server-recipe.txt", "sha256": sha(server_license)},
        ],
        "transforms": [
            "omit snapshotter/resizer containers; no snapshot or resize coverage claimed",
            "bound provisioner operation timeout to 60s",
            "replace all image tags with immutable index digests",
        ],
        "reviewed_on": "2026-11-10",
    }
    path = DEST / "lock.json"
    path.write_text(json.dumps(lock, indent=2) + "\n")
    versions_path = ROOT / "dev/versions.json"
    versions = json.loads(versions_path.read_text())
    versions.update(nfs_csi_release=RELEASE, nfs_lock_sha256=sha(path.read_bytes()))
    versions_path.write_text(json.dumps(versions, indent=2) + "\n")


if __name__ == "__main__":
    main()
