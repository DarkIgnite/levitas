"""Download the official 1.21.8 server.jar and run its data generator.

Output goes to generated/1.21.8/ (ignored by git, see docs/CLEAN_ROOM.md):
  reports/       blocks.json, registries.json, packets.json, ...
  data/          data-driven registries and tags
  version.json   the jar's version info (data version, protocol, pack formats)
  minecraft-data/  block hardness and collision shapes from PrismarineJS minecraft-data
                   (MIT licensed), which the official generator does not output

Requires Java 21+. Usage: python tools/datagen.py
"""

import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import urllib.request
import zipfile

VERSION = "1.21.8"
MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
# PrismarineJS/minecraft-data, pinned to a commit, with the expected SHA-256 of each file.
MINECRAFT_DATA = "https://raw.githubusercontent.com/PrismarineJS/minecraft-data/5e063fa99cfe8ded065ec6788b66696b7e4ef143/data/pc/1.21.8/"
MINECRAFT_DATA_FILES = {
    "blocks.json": "197629a9e61f721462300cbb6d9d8fe6202a00694b80be6860517490632baa70",
    "blockCollisionShapes.json": "b13047ae9a025ffed07bf6f5749f5addfe177148d32816b1e32aefefca6231a6",
}

root = pathlib.Path(__file__).resolve().parent.parent
out = root / "generated" / VERSION
jar = root / "generated" / f"server-{VERSION}.jar"


def fetch_json(url):
    with urllib.request.urlopen(url) as r:
        return json.load(r)


def sha1(path):
    return hashlib.sha1(path.read_bytes()).hexdigest()


def fetch_minecraft_data():
    target = out / "minecraft-data"
    target.mkdir(parents=True, exist_ok=True)
    for name, expected in MINECRAFT_DATA_FILES.items():
        print(f"downloading minecraft-data {name}")
        with urllib.request.urlopen(MINECRAFT_DATA + name) as r:
            data = r.read()
        if hashlib.sha256(data).hexdigest() != expected:
            sys.exit(f"minecraft-data {name} checksum mismatch")
        (target / name).write_bytes(data)


def main():
    manifest = fetch_json(MANIFEST)
    version_url = next(v["url"] for v in manifest["versions"] if v["id"] == VERSION)
    server = fetch_json(version_url)["downloads"]["server"]

    if not jar.exists() or sha1(jar) != server["sha1"]:
        print(f"downloading {server['url']}")
        jar.parent.mkdir(parents=True, exist_ok=True)
        urllib.request.urlretrieve(server["url"], jar)
        if sha1(jar) != server["sha1"]:
            sys.exit("server.jar checksum mismatch")

    if out.exists():
        shutil.rmtree(out)
    # The generator writes logs and libraries into the working directory.
    work = root / "generated" / "work"
    work.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["java", "-DbundlerMainClass=net.minecraft.data.Main", "-jar", str(jar),
         "--reports", "--server", "--output", str(out)],
        cwd=work, check=True,
    )
    with zipfile.ZipFile(jar) as z:
        (out / "version.json").write_bytes(z.read("version.json"))
    fetch_minecraft_data()
    print(f"data written to {out}")


if __name__ == "__main__":
    main()
