#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = ["pyyaml==6.0.3"]
# ///
"""Reproduce the reviewed CSI JSON bundle from exact local upstream checkouts.

This review-time tool reads registry manifests; cluster setup uses JSON/std only.
Read README.md before changing the selected sources or transformations.
"""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

import yaml

ROOT = Path(__file__).resolve().parents[2]
DESTINATION = ROOT / "dev/csi"
UPSTREAM = ROOT / ".local/upstream"
SOURCES = {
    "csi-driver-host-path": ("v1.19.0", "6fab18ae4fc4f486193b618114eeb817d9929242"),
    "external-attacher": ("v4.13.0", "40d44ae14961a415348180a0e666651e82e44eb7"),
    "external-provisioner": ("v6.4.0", "72f13ee46fb93c11a72ef91549e70158283c03af"),
    "external-resizer": ("v2.3.0", "baa08e6d8773f167dbddce36579f36d84bbe3fde"),
    "external-snapshotter": ("v8.7.0", "9cc6050ab00a0c56d84ba0279e518a00367363e7"),
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def command(*args):
    return subprocess.check_output(args, text=True, timeout=90)


def documents(source, filename, provenance):
    path = UPSTREAM / source / filename
    provenance[source]["files"].append({"file": filename, "sha256": digest(path.read_bytes())})
    return [value for value in yaml.safe_load_all(path.read_text()) if value is not None]


def registry_image(reference):
    # Resolve the tag once, then obtain platforms by that immutable index digest.
    description = command("docker", "buildx", "imagetools", "inspect", reference)
    match = re.search(r"^Digest:\s+(sha256:[0-9a-f]{64})\s*$", description, re.MULTILINE)
    if not match:
        raise RuntimeError(f"registry did not return an index digest for {reference}")
    immutable = reference + "@" + match[1]
    index = json.loads(command("docker", "buildx", "imagetools", "inspect", "--raw", immutable))
    platforms = {}
    for arch in ("amd64", "arm64"):
        matches = [entry["digest"] for entry in index["manifests"]
                   if entry["platform"].get("os") == "linux" and entry["platform"].get("architecture") == arch]
        if len(matches) != 1 or not re.fullmatch(r"sha256:[0-9a-f]{64}", matches[0]):
            raise RuntimeError(f"registry lacks one unambiguous linux/{arch} manifest for {reference}")
        platforms[arch] = matches[0]
    print("VERIFIED registry index/platforms: " + immutable, flush=True)
    return {"image": immutable, "platform_digests": platforms}


def container_images(value):
    if isinstance(value, dict):
        for key, child in value.items():
            if key in ("containers", "initContainers", "ephemeralContainers"):
                for container in child:
                    yield container
            yield from container_images(child)
    elif isinstance(value, list):
        for child in value:
            yield from container_images(child)


def main():
    versions = json.loads((ROOT / "dev/versions.json").read_text())
    if versions["csi_hostpath_release"] != SOURCES["csi-driver-host-path"][0]:
        raise RuntimeError("review the source selection when changing the hostpath release")
    provenance = {}
    for source, (tag, revision) in SOURCES.items():
        actual = command("git", "-C", str(UPSTREAM / source), "rev-parse", "HEAD").strip()
        if actual != revision or command("git", "-C", str(UPSTREAM / source), "status", "--porcelain").strip():
            raise RuntimeError(f"source checkout must be clean at the reviewed {source} commit")
        provenance[source] = {"repository": "https://github.com/kubernetes-csi/" + source,
                              "tag": tag, "revision": revision, "files": []}
    base = "deploy/kubernetes-1.35/hostpath/"
    driver = documents("csi-driver-host-path", base + "csi-hostpath-plugin.yaml", provenance)
    # Optional snapshot-metadata service is disabled in the upstream deployment.
    driver = [item for item in driver if not item.get("roleRef", {}).get("name", "").startswith("external-snapshot-metadata")]
    driver = documents("csi-driver-host-path", base + "csi-hostpath-driverinfo.yaml", provenance) + driver
    for container in container_images(driver):
        if container["name"] == "hostpath":
            if container["image"] != "registry.k8s.io/sig-storage/hostpathplugin:v1.18.0":
                raise RuntimeError("review changed upstream hostpath image selection")
            container["image"] = versions["csi_hostpath_image"]
    roles = []
    for source, file in [("external-attacher", "deploy/kubernetes/rbac.yaml"),
                         ("external-provisioner", "deploy/kubernetes/rbac.yaml"),
                         ("external-resizer", "deploy/kubernetes/rbac.yaml"),
                         ("external-snapshotter", "deploy/kubernetes/csi-snapshotter/rbac-csi-snapshotter.yaml")]:
        # The hostpath bundle supplies bindings to its one selected service account.
        roles.extend(item for item in documents(source, file, provenance) if item["kind"] in ("Role", "ClusterRole"))
    role_names = {(item["kind"], item["metadata"]["name"]) for item in roles}
    if any((item["roleRef"]["kind"], item["roleRef"]["name"]) not in role_names for item in driver if "roleRef" in item):
        raise RuntimeError("driver binding references an unreviewed/missing role")
    snapshot = "deploy/kubernetes/snapshot-controller/"
    controller = documents("external-snapshotter", snapshot + "rbac-snapshot-controller.yaml", provenance)
    controller += documents("external-snapshotter", snapshot + "setup-snapshot-controller.yaml", provenance)
    for container in container_images(controller):
        if container["image"] != "registry.k8s.io/sig-storage/snapshot-controller:v8.6.0":
            raise RuntimeError("review changed upstream snapshot-controller image selection")
        container["image"] = "registry.k8s.io/sig-storage/snapshot-controller:v8.7.0"
    crds = []
    for resource in ("volumesnapshotclasses", "volumesnapshotcontents", "volumesnapshots"):
        crds += documents("external-snapshotter", f"client/config/crd/snapshot.storage.k8s.io_{resource}.yaml", provenance)
    bundles = [("snapshot-crds", crds), ("driver-rbac", roles), ("snapshot-controller", controller), ("hostpath", driver)]
    references = {container["image"] for _, items in bundles for container in container_images(items)}
    references.add(versions["csi_workload_image"])
    with ThreadPoolExecutor(max_workers=4) as pool:
        images = list(pool.map(registry_image, sorted(references)))
    locked = {entry["image"].split("@")[0]: entry["image"] for entry in images}
    manifests = []
    (DESTINATION / "vendor").mkdir(exist_ok=True)
    for name, items in bundles:
        for container in container_images(items):
            container["image"] = locked[container["image"]]
        file = DESTINATION / "vendor" / (name + ".json")
        file.write_text(json.dumps({"apiVersion": "v1", "kind": "List", "items": items}, indent=2) + "\n")
        manifests.append({"file": str(file.relative_to(DESTINATION)), "sha256": digest(file.read_bytes())})
    licenses = []
    (DESTINATION / "licenses").mkdir(exist_ok=True)
    for source in SOURCES:
        original = UPSTREAM / source / "LICENSE"
        file = DESTINATION / "licenses" / (source + ".txt")
        shutil.copyfile(original, file)
        licenses.append({"file": str(file.relative_to(DESTINATION)), "sha256": digest(file.read_bytes())})
    lock = {"status": "reviewed", "driver_release": versions["csi_hostpath_release"],
            "source_revision": SOURCES["csi-driver-host-path"][1],
            "snapshot_dependency_revisions": {"external-snapshotter": SOURCES["external-snapshotter"][1]},
            "driver_image": locked[versions["csi_hostpath_image"]], "workload_image": locked[versions["csi_workload_image"]],
            "images": images, "manifests": manifests, "licenses": licenses,
            "snapshot_controller": {"namespace": "kube-system", "name": "snapshot-controller"},
            "sources": provenance, "yaml_review_parser": "PyYAML " + yaml.__version__,
            "reviewed_on": "2026-11-07"}
    file = DESTINATION / "lock.json"
    file.write_text(json.dumps(lock, indent=2) + "\n")
    versions["csi_lock_sha256"] = digest(file.read_bytes())
    (ROOT / "dev/versions.json").write_text(json.dumps(versions, indent=2) + "\n")
    sys.path.insert(0, str(ROOT / "scripts"))
    import dev_csi
    dev_csi.validate_lock(DESTINATION, versions)
    print("PASS reviewed CSI lock consistency; live rollout and telemetry remain separate gates", flush=True)


if __name__ == "__main__":
    main()
