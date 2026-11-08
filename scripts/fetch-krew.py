#!/usr/bin/env python3
"""Install a verified Krew test tool locally; never change the user's KREW_ROOT."""
import hashlib
import io
import json
from pathlib import Path
import platform
import tarfile
import urllib.request

ROOT=Path(__file__).resolve().parents[1]


def main():
    selected=json.loads((ROOT/"dev/versions.json").read_text())["krew_validation"]
    system={"Darwin":"darwin","Linux":"linux"}[platform.system()]
    arch={"arm64":"arm64","aarch64":"arm64","x86_64":"amd64","amd64":"amd64"}[platform.machine().lower()]
    target=f"{system}_{arch}"
    asset=selected["assets"][target]
    expected_url=f"https://github.com/kubernetes-sigs/krew/releases/download/{selected['version']}/krew-{target}.tar.gz"
    if asset["url"]!=expected_url:
        raise RuntimeError("Krew tool must use the reviewed official release URL")
    tool=ROOT/".local/tools"
    (tool/"bin").mkdir(parents=True,exist_ok=True)
    (tool/"licenses").mkdir(parents=True,exist_ok=True)
    receipt={"status":"running","release":selected["version"],"platform":target,"source":asset["url"],"archive_sha256":asset["sha256"],"scope":"workspace-local test tool; user Krew installation unchanged"}
    result=tool/"krew-verification.json"
    result.write_text(json.dumps(receipt,indent=2)+"\n")
    try:
        with urllib.request.urlopen(asset["url"],timeout=90) as response:
            data=response.read(64*1024*1024+1)
        if len(data)!=asset["bytes"] or len(data)>64*1024*1024 or hashlib.sha256(data).hexdigest()!=asset["sha256"]:
            raise RuntimeError("Krew tool archive checksum/size mismatch")
        with tarfile.open(fileobj=io.BytesIO(data),mode="r:gz") as archive:
            def member(name):
                members=[m for m in archive.getmembers() if m.isfile() and Path(m.name).name==name]
                if len(members)!=1 or members[0].size>32*1024*1024:
                    raise RuntimeError("Krew tool archive lacks one bounded regular member")
                return archive.extractfile(members[0]).read()
            binary=member("krew-"+target)
            license_data=member("LICENSE")
        destination=tool/"bin/kubectl-krew"
        license_path=tool/"licenses/krew-LICENSE"
        if destination.is_symlink() or license_path.is_symlink():
            raise RuntimeError("Krew tool destination must not be a symlink")
        destination.write_bytes(binary)
        destination.chmod(0o755)
        license_path.write_bytes(license_data)
        receipt.update(status="passed",binary_sha256=hashlib.sha256(binary).hexdigest())
    except BaseException as error:
        receipt.update(status="failed",error_kind=type(error).__name__)
        raise
    finally:
        result.write_text(json.dumps(receipt,indent=2)+"\n")
    print(f"PASS checksum-verified workspace-local Krew {selected['version']} ({target})")


if __name__=="__main__":
    main()
