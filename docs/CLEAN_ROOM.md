# Clean-room policy

Levitas is GPL-3.0 open source. To keep it legally distributable, every contributor
(human or AI) follows these rules.

## Allowed sources

- Public protocol documentation (minecraft.wiki "Java Edition protocol" pages, the former wiki.vg).
- JSON reports from the official server's data generator, produced locally at build time:
  `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --all`
- Observing a running vanilla/Leaf server: packet captures, in-game behaviour, timings.
- The Minecraft Wiki's descriptions of game mechanics.
- Open-source code under GPL-3.0-compatible licenses, with attribution.

## Not allowed

- Copying, translating, or paraphrasing code from decompiled Minecraft, or from the
  patched sources of Paper / Purpur / Leaf / Folia (these contain Mojang code).
- Committing Mojang assets or generated data files to the repository. Generated data
  lives in an ignored directory and is produced from a `server.jar` the user supplies.
- Using "Minecraft" or Mojang trademarks in project names, crate names, or logos.

## Plugins

Ports of Bukkit/Paper plugins are only done from plugins whose license allows it, and the
port keeps the original license and credits. Closed-source or premium plugins are never
decompiled; a replacement is written from a description of its features instead.
