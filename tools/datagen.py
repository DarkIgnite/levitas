"""Download the official 1.21.8 server.jar and run its data generator.

Output goes to generated/1.21.8/ (ignored by git, see docs/CLEAN_ROOM.md):
  reports/   blocks.json, registries.json, packets.json, ...
  data/      data-driven registries and tags

Requires Java 21+. Usage: python tools/datagen.py
"""

import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import urllib.request

VERSION = "1.21.8"
MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"

root = pathlib.Path(__file__).resolve().parent.parent
out = root / "generated" / VERSION
jar = root / "generated" / f"server-{VERSION}.jar"


def fetch_json(url):
    with urllib.request.urlopen(url) as r:
        return json.load(r)


def sha1(path):
    return hashlib.sha1(path.read_bytes()).hexdigest()


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
    print(f"data written to {out}")


if __name__ == "__main__":
    main()
