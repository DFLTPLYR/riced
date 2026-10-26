"""Regenerate the pinned catalog from a checksum-verified Lucide release.

Run `python3 scripts/update_lucide.py [ARCHIVE]`. Without ARCHIVE the pinned
release is downloaded. Ordinary Cargo builds never run this script or fetch
icons. To upgrade, deliberately change VERSION, SHA256, and EXPECTED_ICONS.
"""
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import urllib.request

VERSION = "1.61.0"
SHA256 = "3c507183f93fa66664e25210e482b4fc88aaa3ee6702a4a9aa11cd51f9d22df5"
EXPECTED_ICONS = 1903
URL = f"https://codeload.github.com/lucide-icons/lucide/tar.gz/refs/tags/{VERSION}"


def main():
    if len(sys.argv) > 2:
        raise SystemExit("usage: update_lucide.py [ARCHIVE]")
    if len(sys.argv) == 2:
        data = Path(sys.argv[1]).read_bytes()
    else:
        with urllib.request.urlopen(URL, timeout=90) as response:
            data = response.read()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise SystemExit("Lucide archive checksum mismatch; no files written")
    prefix = f"lucide-{VERSION}/icons/"
    icons = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for entry in archive.getmembers():
            if not entry.isfile() or not entry.name.startswith(prefix):
                continue
            name = entry.name[len(prefix):]
            if "/" in name or not name.endswith(".svg"):
                continue
            if entry.size > 65536 or name[:-4] in icons:
                raise SystemExit(f"invalid icon entry {name}; no files written")
            icons[name[:-4]] = archive.extractfile(entry).read().decode("utf-8")
        license_text = archive.extractfile(f"lucide-{VERSION}/LICENSE").read().decode("utf-8")
    if len(icons) != EXPECTED_ICONS:
        raise SystemExit(f"unexpected icon count: {len(icons)}; no files written")
    destination = Path(__file__).resolve().parent.parent / "assets/lucide"
    destination.mkdir(parents=True, exist_ok=True)
    catalog = dict(version=VERSION, archive_sha256=SHA256, icons=dict(sorted(icons.items())))
    (destination / "icons.json").write_text(json.dumps(catalog, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    (destination / "LICENSE").write_text(license_text, encoding="utf-8")
    print(f"Pinned Lucide {VERSION}: {len(icons)} icons")


if __name__ == "__main__":
    main()
